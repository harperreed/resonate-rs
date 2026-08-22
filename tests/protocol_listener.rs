mod common;

use common::MockServer;
use sendspin::protocol::messages::{Activity, ClientCommand, Message};
use sendspin::ProtocolClientBuilder;
use std::sync::Arc;
use tokio::time::{timeout, Duration};
use tokio_tungstenite::connect_async;

async fn listener(path: Option<&str>) -> sendspin::ProtocolListener {
    let listener = ProtocolClientBuilder::builder()
        .name("Inbound Client".into())
        .build()
        .listen("127.0.0.1:0")
        .await
        .unwrap();
    match path {
        Some(path) => listener.path(path),
        None => listener,
    }
}

#[tokio::test]
async fn listener_accepts_server_initiated_noise_connection() {
    let listener = listener(None).await;
    let addr = listener.local_addr().unwrap();
    let accept_task = tokio::spawn(async move { listener.accept().await });
    let server = MockServer::dial(
        addr,
        "Inbound Server",
        vec![Activity::Playback],
        vec!["player@v1".into()],
    )
    .await
    .unwrap();
    let (client, peer) = timeout(Duration::from_secs(5), accept_task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(peer.ip(), addr.ip());
    assert_eq!(client.session().server_name, "Inbound Server");
    assert_eq!(
        client.session().initial_activities,
        vec![Activity::Playback]
    );
    assert_eq!(client.session().initial_active_roles, vec!["player@v1"]);
    assert_eq!(server.server_id.len(), 43);
}

#[tokio::test]
async fn listener_path_match_accepts() {
    let listener = listener(Some("/sendspin")).await;
    let addr = listener.local_addr().unwrap();
    let accept_task = tokio::spawn(async move { listener.accept().await });
    let server = MockServer::dial(addr, "Matched", vec![Activity::Playback], vec![])
        .await
        .unwrap();
    let (client, _) = timeout(Duration::from_secs(5), accept_task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(client.session().server_name, "Matched");
    drop(server);
}

#[tokio::test]
async fn listener_path_normalizes_missing_leading_slash() {
    let listener = listener(Some("sendspin")).await;
    let addr = listener.local_addr().unwrap();
    let accept_task = tokio::spawn(async move { listener.accept().await });
    let server = MockServer::dial(addr, "Normalized", vec![Activity::Playback], vec![])
        .await
        .unwrap();
    let (client, _) = timeout(Duration::from_secs(5), accept_task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(client.session().server_name, "Normalized");
    drop(server);
}

#[tokio::test]
async fn listener_wrong_path_rejects_but_survives() {
    let listener = Arc::new(listener(Some("/sendspin")).await);
    let addr = listener.local_addr().unwrap();
    let wrong_listener = Arc::clone(&listener);
    let wrong_accept = tokio::spawn(async move { wrong_listener.accept().await });
    let wrong = timeout(
        Duration::from_secs(3),
        connect_async(format!("ws://{addr}/wrong")),
    )
    .await
    .expect("wrong-path handshake hung")
    .expect_err("wrong path unexpectedly completed WebSocket handshake");
    let _ = wrong;
    let wrong_result = timeout(Duration::from_secs(3), wrong_accept)
        .await
        .expect("wrong-path accept hung")
        .expect("wrong-path accept task panicked");
    assert!(wrong_result.is_err(), "wrong path unexpectedly accepted");

    let accept_listener = Arc::clone(&listener);
    let accept_task = tokio::spawn(async move { accept_listener.accept().await });
    let server = MockServer::dial(addr, "After Wrong Path", vec![Activity::Playback], vec![])
        .await
        .unwrap();
    let (client, _) = timeout(Duration::from_secs(5), accept_task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(client.session().server_name, "After Wrong Path");
    drop(server);
}

#[tokio::test]
async fn listener_local_addr_is_ephemeral_bound_address() {
    let listener = listener(None).await;
    let addr = listener.local_addr().unwrap();
    assert_eq!(addr.ip(), "127.0.0.1".parse::<std::net::IpAddr>().unwrap());
    assert_ne!(addr.port(), 0);
}

#[tokio::test]
async fn sends_fail_after_server_disconnects() {
    let listener = listener(None).await;
    let addr = listener.local_addr().unwrap();
    let accept_task = tokio::spawn(async move { listener.accept().await });
    let server = MockServer::dial(addr, "Disconnecting", vec![Activity::Playback], vec![])
        .await
        .unwrap();
    let (client, _) = timeout(Duration::from_secs(5), accept_task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    drop(server);

    let mut connection = client.split();
    timeout(Duration::from_secs(3), async {
        while connection.messages.recv().await.is_some() {}
    })
    .await
    .expect("client did not observe server disconnect");
    let send = timeout(
        Duration::from_secs(3),
        connection
            .sender
            .send_message(Message::ClientCommand(ClientCommand { controller: None })),
    )
    .await
    .expect("send remained pending after server disconnect");
    assert!(
        send.is_err(),
        "send unexpectedly succeeded after disconnect"
    );
}
