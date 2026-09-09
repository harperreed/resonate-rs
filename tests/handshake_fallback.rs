// ABOUTME: Handshake-level tests for psk_category selection, the Sentinel
// ABOUTME: Fallback, and the long-term record server_id binding check

use futures_util::{SinkExt, StreamExt};
use sendspin::protocol::crypto::{
    b64url_decode, b64url_decode_32, b64url_encode, build_server_handshake, CipherSuite, Identity,
    Psk, PskCandidate, PskCategory,
};
use sendspin::protocol::messages::{
    Activity, ClientInit, Message, NoiseHandshake, ServerActivate, ServerHello, ServerInit,
};
use sendspin::protocol::transport::frame_type;
use sendspin::{ClientCredentials, ProtocolClientBuilder};
use std::error::Error;
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;

type BoxError = Box<dyn Error + Send + Sync>;

/// Run a minimal raw server for one connection: complete the cleartext init
/// exchange, send Noise message 1 declaring (`psk_id`, `psk_category`) while
/// actually mixing `handshake_psk`, and — if the handshake completes — the
/// encrypted server/hello + server/activate ([], []).
///
/// Returns Ok(true) when the whole sequence completed, Ok(false) when the
/// client walked away before Noise message 2 (a failed handshake).
async fn run_server(
    listener: TcpListener,
    declared_psk_id: String,
    declared_category: &str,
    handshake_psk: Psk,
) -> Result<bool, BoxError> {
    let (stream, _) = listener.accept().await?;
    let mut ws = accept_async(stream).await?;

    let client_init_wire = match ws.next().await.ok_or("closed before client/init")?? {
        WsMessage::Text(text) => text.to_string(),
        other => return Err(format!("expected client/init text frame, got {other:?}").into()),
    };
    let Message::ClientInit(ClientInit {
        client_id,
        suite: suite_name,
        ..
    }) = serde_json::from_str(&client_init_wire)?
    else {
        return Err("expected client/init".into());
    };
    let suite = CipherSuite::from_wire_name(&suite_name)?;
    let client_public = b64url_decode_32(&client_id)?;
    let server_identity = Identity::generate()?;
    let server_init_wire = serde_json::to_string(&Message::ServerInit(ServerInit {
        server_id: server_identity.id(),
        version: 1,
    }))?;
    ws.send(WsMessage::Text(server_init_wire.clone().into()))
        .await?;

    let mut prologue = client_init_wire.into_bytes();
    prologue.extend_from_slice(server_init_wire.as_bytes());
    let mut handshake = build_server_handshake(
        suite,
        &server_identity,
        &client_public,
        &handshake_psk,
        &prologue,
    )?;
    let mut buf = vec![0u8; 65535];
    let payload =
        format!(r#"{{"psk_id":"{declared_psk_id}","psk_category":"{declared_category}"}}"#);
    let len = handshake.write_message(payload.as_bytes(), &mut buf)?;
    let msg1 = Message::NoiseHandshake(NoiseHandshake {
        data: b64url_encode(&buf[..len]),
    });
    ws.send(WsMessage::Text(serde_json::to_string(&msg1)?.into()))
        .await?;

    // A client that rejects the handshake closes without message 2.
    let msg2_wire = loop {
        match ws.next().await {
            Some(Ok(WsMessage::Text(text))) => break text.to_string(),
            Some(Ok(WsMessage::Ping(_))) | Some(Ok(WsMessage::Pong(_))) => continue,
            Some(Ok(WsMessage::Close(_))) | None => return Ok(false),
            Some(Ok(other)) => return Err(format!("unexpected frame {other:?}").into()),
            Some(Err(_)) => return Ok(false),
        }
    };
    let Message::NoiseHandshake(msg2) = serde_json::from_str(&msg2_wire)? else {
        return Err("expected noise message 2".into());
    };
    let msg2_bytes = b64url_decode(&msg2.data)?;
    let mut payload = vec![0u8; 65535];
    handshake.read_message(&msg2_bytes, &mut payload)?;
    let mut transport = handshake.into_transport_mode()?;

    let send_json = |transport: &mut snow::TransportState, message: &Message| {
        let json = serde_json::to_vec(message).unwrap();
        let mut plain = Vec::with_capacity(json.len() + 1);
        plain.push(frame_type::JSON);
        plain.extend_from_slice(&json);
        let mut encrypted = vec![0u8; plain.len() + 32];
        let n = transport.write_message(&plain, &mut encrypted).unwrap();
        encrypted.truncate(n);
        encrypted
    };

    let hello = send_json(
        &mut transport,
        &Message::ServerHello(ServerHello {
            name: "fallback-test".to_string(),
            languages: None,
        }),
    );
    ws.send(WsMessage::Binary(hello.into())).await?;

    // client/hello arrives encrypted; decrypt (and discard) frames until it
    // parses, then activate with the empty set.
    loop {
        match ws.next().await.ok_or("closed before client/hello")?? {
            WsMessage::Binary(bytes) => {
                let mut plain = vec![0u8; 65535];
                let n = transport.read_message(&bytes, &mut plain)?;
                if n > 0 && plain[0] == frame_type::JSON {
                    if let Ok(Message::ClientHello(_)) =
                        serde_json::from_slice::<Message>(&plain[1..n])
                    {
                        break;
                    }
                }
            }
            WsMessage::Ping(_) | WsMessage::Pong(_) => continue,
            other => return Err(format!("expected client/hello, got {other:?}").into()),
        }
    }
    let activate = send_json(
        &mut transport,
        &Message::ServerActivate(ServerActivate {
            activities: vec![Activity::Playback],
            active_roles: Some(vec![]),
            pairing: None,
        }),
    );
    ws.send(WsMessage::Binary(activate.into())).await?;
    Ok(true)
}

/// A psk_id the client does not hold (declared as a long-term PSK) triggers
/// the Sentinel Fallback: the client completes the handshake with the
/// Sentinel PSK and the session proceeds unpaired.
#[tokio::test]
async fn lookup_miss_falls_back_to_sentinel() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let unknown_psk_id = Psk::new([0x42; 32]).psk_id();
    let server = tokio::spawn(run_server(listener, unknown_psk_id, "lt", Psk::sentinel()));

    let client = ProtocolClientBuilder::builder()
        .credentials(ClientCredentials::generate().unwrap())
        .name("fallback client".to_string())
        .build()
        .connect(format!("ws://{addr}/sendspin"))
        .await
        .expect("Sentinel Fallback must complete the handshake");
    let conn = client.split();
    assert!(!conn.session.paired, "fallback session must be unpaired");
    assert!(server.await.unwrap().unwrap());
}

/// A psk_id the client holds — but only under a different category — is a
/// lookup miss, which in the initial handshake also lands in the fallback.
#[tokio::test]
async fn category_mismatch_is_a_miss_and_falls_back() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    // The client's own pairing PSK, but declared as a long-term PSK.
    let pairing_psk = Psk::new([0x21; 32]);
    let server = tokio::spawn(run_server(
        listener,
        pairing_psk.psk_id(),
        "lt",
        Psk::sentinel(),
    ));

    let client = ProtocolClientBuilder::builder()
        .credentials(ClientCredentials::from_parts(
            Identity::generate().unwrap(),
            pairing_psk,
        ))
        .name("category client".to_string())
        .build()
        .connect(format!("ws://{addr}/sendspin"))
        .await
        .expect("category-scoped miss must fall back to the Sentinel");
    let conn = client.split();
    assert!(!conn.session.paired);
    assert!(server.await.unwrap().unwrap());
}

/// A matched long-term record bound to a different server_id is a
/// misbinding, not a miss: the handshake fails (no fallback) even though the
/// server holds the PSK.
#[tokio::test]
async fn misbound_long_term_record_fails_handshake() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let stolen_psk = Psk::new([0x77; 32]);
    let server = tokio::spawn(run_server(
        listener,
        stolen_psk.psk_id(),
        "lt",
        stolen_psk.clone(),
    ));

    let other_server_id = Identity::generate().unwrap().id();
    let result = ProtocolClientBuilder::builder()
        .credentials(ClientCredentials::generate().unwrap())
        .name("misbinding client".to_string())
        .psk_records(vec![PskCandidate {
            psk: stolen_psk,
            category: PskCategory::LongTerm {
                server_id: other_server_id,
            },
        }])
        .build()
        .connect(format!("ws://{addr}/sendspin"))
        .await;
    assert!(result.is_err(), "misbinding must fail the handshake");
    assert!(
        !server.await.unwrap().unwrap(),
        "client must close before Noise message 2"
    );
}
