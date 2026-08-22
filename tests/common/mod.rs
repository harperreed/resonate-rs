#![allow(dead_code)] // shared harness: not every test binary uses every helper
#![allow(clippy::large_enum_variant)]
use futures_util::{SinkExt, StreamExt};
use sendspin::protocol::crypto::{
    b64url_decode, b64url_decode_32, b64url_encode, build_server_handshake, CipherSuite, Identity,
    Psk,
};
use sendspin::protocol::messages::{
    Activity, ClientHello, ClientInit, ClientTime, Message, NoiseHandshake, ServerActivate,
    ServerHello, ServerInit, ServerTime,
};
use sendspin::protocol::transport::frame_type;
use std::error::Error;
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::{accept_async, connect_async, WebSocketStream};

type BoxError = Box<dyn Error + Send + Sync>;
type Ws<S> = WebSocketStream<S>;

enum Command {
    Json(Message),
    Audio { timestamp: i64, data: Vec<u8> },
}

/// Reusable spec-1.0 server-side Noise/WebSocket fixture.
pub struct MockServer {
    pub server_id: String,
    /// The capabilities sent by the client during the encrypted handshake.
    pub client_hello: ClientHello,
    commands: mpsc::UnboundedSender<Command>,
    incoming: mpsc::UnboundedReceiver<Message>,
    closed: mpsc::UnboundedReceiver<()>,
}

impl MockServer {
    /// Accept one client connection from a bound TCP listener.
    pub async fn accept(
        listener: TcpListener,
        name: &str,
        activities: Vec<Activity>,
        active_roles: Vec<String>,
    ) -> Result<Self, BoxError> {
        let (stream, _) = listener.accept().await?;
        let ws = accept_async(stream).await?;
        Self::handshake(ws, name, activities, active_roles).await
    }

    /// Dial a client listener and complete the server side of the protocol.
    pub async fn dial(
        addr: SocketAddr,
        name: &str,
        activities: Vec<Activity>,
        active_roles: Vec<String>,
    ) -> Result<Self, BoxError> {
        let (ws, _) = connect_async(format!("ws://{addr}/sendspin")).await?;
        Self::handshake(ws, name, activities, active_roles).await
    }

    /// Send an encrypted JSON protocol message to the client.
    pub async fn send_json(&self, message: Message) -> Result<(), BoxError> {
        self.commands
            .send(Command::Json(message))
            .map_err(|_| "mock server task stopped".into())
    }

    /// Send an encrypted type-4 audio chunk.
    pub async fn send_audio(&self, timestamp: i64, data: &[u8]) -> Result<(), BoxError> {
        self.commands
            .send(Command::Audio {
                timestamp,
                data: data.to_vec(),
            })
            .map_err(|_| "mock server task stopped".into())
    }

    /// Receive the next client JSON message, ignoring periodic client/time.
    pub async fn recv_json(&mut self) -> Result<Message, BoxError> {
        self.incoming
            .recv()
            .await
            .ok_or_else(|| "mock server task stopped".into())
    }

    /// Wait until the client has observed the WebSocket close.
    pub async fn recv_closed(&mut self) -> bool {
        self.closed.recv().await.is_some()
    }

    async fn handshake<S>(
        mut ws: Ws<S>,
        name: &str,
        activities: Vec<Activity>,
        active_roles: Vec<String>,
    ) -> Result<Self, BoxError>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let client_init_wire = next_text(&mut ws).await?;
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

        let mut prologue = client_init_wire.as_bytes().to_vec();
        prologue.extend_from_slice(server_init_wire.as_bytes());
        let mut handshake = build_server_handshake(
            suite,
            &server_identity,
            &client_public,
            &Psk::sentinel(),
            &prologue,
        )?;
        let mut buf = vec![0u8; 65535];
        let msg1_len = handshake.write_message(
            format!(r#"{{"psk_id":"{}"}}"#, Psk::sentinel().psk_id()).as_bytes(),
            &mut buf,
        )?;
        let msg1 = Message::NoiseHandshake(NoiseHandshake {
            data: b64url_encode(&buf[..msg1_len]),
        });
        ws.send(WsMessage::Text(serde_json::to_string(&msg1)?.into()))
            .await?;

        let msg2_wire = next_text(&mut ws).await?;
        let Message::NoiseHandshake(msg2) = serde_json::from_str(&msg2_wire)? else {
            return Err("expected noise/handshake message 2".into());
        };
        let msg2_bytes = b64url_decode(&msg2.data)?;
        let mut payload = vec![0u8; 65535];
        handshake.read_message(&msg2_bytes, &mut payload)?;
        let mut transport = handshake.into_transport_mode()?;

        let server_hello = Message::ServerHello(ServerHello {
            name: name.to_string(),
        });
        let json = serde_json::to_vec(&server_hello)?;
        let mut plain = Vec::with_capacity(json.len() + 1);
        plain.push(frame_type::JSON);
        plain.extend_from_slice(&json);
        let mut encrypted = vec![0u8; plain.len() + 32];
        let n = transport.write_message(&plain, &mut encrypted)?;
        encrypted.truncate(n);
        ws.send(WsMessage::Binary(encrypted.into())).await?;

        let client_hello = loop {
            match ws
                .next()
                .await
                .ok_or("websocket closed before client/hello")??
            {
                WsMessage::Binary(bytes) => {
                    let mut plain = vec![0u8; 65535];
                    let n = transport.read_message(&bytes, &mut plain)?;
                    if n > 0 && plain[0] == frame_type::JSON {
                        let message: Message = serde_json::from_slice(&plain[1..n])?;
                        if let Message::ClientHello(hello) = message {
                            break hello;
                        }
                    }
                }
                WsMessage::Ping(_) | WsMessage::Pong(_) => continue,
                _ => return Err("expected encrypted client/hello".into()),
            }
        };

        let activate = Message::ServerActivate(ServerActivate {
            activities,
            active_roles: Some(active_roles),
            pairing: None,
        });
        let json = serde_json::to_vec(&activate)?;
        let mut plain = Vec::with_capacity(json.len() + 1);
        plain.push(frame_type::JSON);
        plain.extend_from_slice(&json);
        let mut encrypted = vec![0u8; plain.len() + 32];
        let n = transport.write_message(&plain, &mut encrypted)?;
        encrypted.truncate(n);
        ws.send(WsMessage::Binary(encrypted.into())).await?;

        let (commands, mut command_rx) = mpsc::unbounded_channel();
        let (incoming_tx, incoming) = mpsc::unbounded_channel();
        let (closed_tx, closed) = mpsc::unbounded_channel();
        let server_id = server_identity.id();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    command = command_rx.recv() => match command {
                        Some(Command::Json(message)) => {
                            let json = match serde_json::to_vec(&message) { Ok(value) => value, Err(_) => break };
                            let mut plain = Vec::with_capacity(json.len() + 1);
                            plain.push(frame_type::JSON);
                            plain.extend_from_slice(&json);
                            let mut encrypted = vec![0u8; plain.len() + 32];
                            match transport.write_message(&plain, &mut encrypted) {
                                Ok(n) => { encrypted.truncate(n); if ws.send(WsMessage::Binary(encrypted.into())).await.is_err() { break; } }
                                Err(_) => break,
                            }
                        }
                        Some(Command::Audio { timestamp, data }) => {
                            let mut plain = Vec::with_capacity(9 + data.len());
                            plain.push(4);
                            plain.extend_from_slice(&timestamp.to_be_bytes());
                            plain.extend_from_slice(&data);
                            let mut encrypted = vec![0u8; plain.len() + 32];
                            match transport.write_message(&plain, &mut encrypted) {
                                Ok(n) => { encrypted.truncate(n); if ws.send(WsMessage::Binary(encrypted.into())).await.is_err() { break; } }
                                Err(_) => break,
                            }
                        }
                        None => break,
                    },
                    frame = ws.next() => match frame {
                        Some(Ok(WsMessage::Binary(bytes))) => {
                            let mut plain = vec![0u8; 65535];
                            match transport.read_message(&bytes, &mut plain) {
                                Ok(n) if n > 0 && plain[0] == frame_type::JSON => {
                                    match serde_json::from_slice::<Message>(&plain[1..n]) {
                                        Ok(Message::ClientTime(ClientTime { client_transmitted })) => {
                                            let response = Message::ServerTime(ServerTime {
                                                client_transmitted,
                                                server_received: 0,
                                                server_transmitted: 0,
                                            });
                                            if let Ok(json) = serde_json::to_vec(&response) {
                                                let mut response_plain = Vec::with_capacity(json.len() + 1);
                                                response_plain.push(frame_type::JSON);
                                                response_plain.extend_from_slice(&json);
                                                let mut response_encrypted = vec![0u8; response_plain.len() + 32];
                                                match transport.write_message(&response_plain, &mut response_encrypted) {
                                                    Ok(n) => {
                                                        response_encrypted.truncate(n);
                                                        if ws.send(WsMessage::Binary(response_encrypted.into())).await.is_err() { break; }
                                                    }
                                                    Err(_) => break,
                                                }
                                            }
                                        }
                                        Ok(message) => { let _ = incoming_tx.send(message); }
                                        Err(_) => break,
                                    }
                                }
                                Ok(_) => {}
                                Err(_) => break,
                            }
                        }
                        Some(Ok(WsMessage::Close(_))) | None => { let _ = closed_tx.send(()); break; }
                        Some(Ok(_)) => {}
                        Some(Err(_)) => { let _ = closed_tx.send(()); break; }
                    },
                }
            }
        });
        Ok(Self {
            server_id,
            client_hello,
            commands,
            incoming,
            closed,
        })
    }
}

async fn next_text<S>(ws: &mut Ws<S>) -> Result<String, BoxError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    loop {
        match ws.next().await.ok_or("websocket closed")?? {
            WsMessage::Text(text) => return Ok(text.to_string()),
            WsMessage::Ping(_) | WsMessage::Pong(_) => continue,
            _ => return Err("expected websocket text frame".into()),
        }
    }
}
