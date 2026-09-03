mod common;

use common::{test_credentials, MockServer};
use sendspin::protocol::manager::{
    should_switch, ArbitrationState, ConnectionManager, ManagerConfig,
};
use sendspin::protocol::messages::{
    Activity, ClientGoodbye, GoodbyeReason, Message, PairAbort, PairAbortReason,
};
use sendspin::{ProtocolClientBuilder, SessionInfo};
use std::net::SocketAddr;
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;
use tokio::time::{timeout, Duration};

fn incoming(activities: Vec<Activity>, id: &str) -> SessionInfo {
    SessionInfo {
        server_id: id.into(),
        server_name: id.into(),
        paired: false,
        suite: sendspin::CipherSuite::ChaChaPoly,
        initial_activities: activities,
        initial_active_roles: vec![],
    }
}

fn current(activities: Vec<Activity>, id: &str) -> ArbitrationState {
    ArbitrationState {
        server_id: id.into(),
        activities,
        pairing_attempt_in_progress: false,
    }
}

async fn manager(config: Option<ManagerConfig>) -> (SocketAddr, ConnectionManager) {
    let listener = ProtocolClientBuilder::builder()
        .credentials(test_credentials())
        .name("Managed Client".into())
        .build()
        .listen("127.0.0.1:0")
        .await
        .expect("listen");
    let manager = match config {
        Some(config) => ConnectionManager::with_config(listener, config),
        None => ConnectionManager::new(listener),
    };
    let addr = manager.local_addr().expect("manager local address");
    (addr, manager)
}

async fn next_connection(manager: &mut ConnectionManager) -> sendspin::ManagedConnection {
    timeout(Duration::from_secs(5), manager.next_connection())
        .await
        .expect("next_connection timed out")
        .expect("manager stopped")
}

async fn next_farewell(server: &mut MockServer) -> Message {
    loop {
        let message = timeout(Duration::from_secs(5), server.recv_json())
            .await
            .expect("timed out waiting for server farewell")
            .expect("server socket closed before farewell");
        if matches!(message, Message::ClientGoodbye(_) | Message::PairAbort(_)) {
            return message;
        }
    }
}

async fn next_concurrent_pair_abort(server: &mut MockServer) -> Message {
    loop {
        let message = next_farewell(server).await;
        if matches!(
            message,
            Message::PairAbort(PairAbort {
                reason: PairAbortReason::ConcurrentAttempt
            })
        ) {
            return message;
        }
    }
}

async fn channels_close(connection: &mut sendspin::ManagedConnection) {
    loop {
        match timeout(Duration::from_secs(3), connection.messages.recv()).await {
            Ok(None) => return,
            Ok(Some(_)) => continue,
            Err(_) => panic!("managed connection channels never closed"),
        }
    }
}

#[test]
fn activity_priority_controls_switching() {
    assert!(!should_switch(
        &current(vec![Activity::Playback], "a"),
        &incoming(vec![Activity::Pairing], "b"),
        None
    ));
}

#[test]
fn last_played_breaks_equal_priority() {
    assert!(should_switch(
        &current(vec![Activity::Playback], "a"),
        &incoming(vec![Activity::Playback], "b"),
        Some("a")
    ));
}

#[test]
fn pairing_attempt_is_not_displaced() {
    let mut cur = current(vec![Activity::Pairing], "a");
    cur.pairing_attempt_in_progress = true;
    assert!(!should_switch(
        &cur,
        &incoming(vec![Activity::Playback], "b"),
        None
    ));
}

#[tokio::test]
async fn first_server_is_yielded() {
    let (addr, mut manager) = manager(None).await;
    let server = MockServer::dial(addr, "First", vec![Activity::Playback], vec![])
        .await
        .unwrap();
    let connection = next_connection(&mut manager).await;
    assert_eq!(connection.session.server_name, "First");
    assert_eq!(
        connection.session.initial_activities,
        vec![Activity::Playback]
    );
    assert!(connection.peer.ip().is_loopback());
    assert_eq!(server.server_id.len(), 43);
}

#[tokio::test]
async fn playback_displaces_playback() {
    let (addr, mut manager) = manager(None).await;
    let mut first = MockServer::dial(addr, "First", vec![Activity::Playback], vec![])
        .await
        .unwrap();
    let mut old_connection = next_connection(&mut manager).await;
    let second = MockServer::dial(addr, "Second", vec![Activity::Playback], vec![])
        .await
        .unwrap();

    assert!(matches!(
        next_farewell(&mut first).await,
        Message::ClientGoodbye(ClientGoodbye {
            reason: GoodbyeReason::AnotherServer
        })
    ));
    channels_close(&mut old_connection).await;
    let replacement = next_connection(&mut manager).await;
    assert_eq!(replacement.session.server_name, "Second");
    drop(second);
}

#[tokio::test]
async fn lower_priority_incoming_gets_concurrent_attempt_goodbye() {
    let (addr, mut manager) = manager(None).await;
    let incumbent = MockServer::dial(addr, "Playback", vec![Activity::Playback], vec![])
        .await
        .unwrap();
    let _connection = next_connection(&mut manager).await;
    let mut incoming = MockServer::dial(addr, "Discovery", vec![], vec![])
        .await
        .unwrap();

    assert!(matches!(
        next_farewell(&mut incoming).await,
        Message::ClientGoodbye(ClientGoodbye {
            reason: GoodbyeReason::ConcurrentAttempt
        })
    ));
    drop(incumbent);
}

#[tokio::test]
async fn rejected_pairing_incoming_gets_pair_abort() {
    let (addr, mut manager) = manager(None).await;
    let incumbent = MockServer::dial(addr, "Playback", vec![Activity::Playback], vec![])
        .await
        .unwrap();
    let _connection = next_connection(&mut manager).await;
    let mut incoming = MockServer::dial(addr, "Pairing", vec![Activity::Pairing], vec![])
        .await
        .unwrap();

    next_concurrent_pair_abort(&mut incoming).await;
    drop(incumbent);
}

#[tokio::test]
async fn incumbent_death_frees_slot() {
    let (addr, mut manager) = manager(None).await;
    let first = MockServer::dial(addr, "First", vec![Activity::Playback], vec![])
        .await
        .unwrap();
    let mut connection = next_connection(&mut manager).await;
    drop(first);
    channels_close(&mut connection).await;

    let second = MockServer::dial(addr, "Second", vec![Activity::Playback], vec![])
        .await
        .unwrap();
    assert_eq!(
        next_connection(&mut manager).await.session.server_name,
        "Second"
    );
    drop(second);
}

#[tokio::test]
async fn disconnect_sends_goodbye_and_keeps_listening() {
    let (addr, mut manager) = manager(None).await;
    let mut first = MockServer::dial(addr, "First", vec![Activity::Playback], vec![])
        .await
        .unwrap();
    let _connection = next_connection(&mut manager).await;

    timeout(
        Duration::from_secs(5),
        manager.disconnect(GoodbyeReason::UserRequest),
    )
    .await
    .expect("disconnect timed out")
    .expect("disconnect failed");
    assert!(matches!(
        next_farewell(&mut first).await,
        Message::ClientGoodbye(ClientGoodbye {
            reason: GoodbyeReason::UserRequest
        })
    ));

    let second = MockServer::dial(addr, "Second", vec![Activity::Playback], vec![])
        .await
        .unwrap();
    assert_eq!(
        next_connection(&mut manager).await.session.server_name,
        "Second"
    );
    drop(second);
}

#[tokio::test]
async fn dropping_manager_closes_inflight_handshake() {
    let (addr, manager) = manager(Some(ManagerConfig {
        establish_timeout: Duration::from_secs(30),
        max_concurrent_handshakes: 1,
        ..ManagerConfig::default()
    }))
    .await;
    let mut stalled = TcpStream::connect(addr).await.expect("raw TCP connect");
    drop(manager);

    let mut byte = [0u8; 1];
    match timeout(Duration::from_secs(2), stalled.read(&mut byte)).await {
        Ok(Ok(0)) | Ok(Err(_)) => {}
        Ok(Ok(n)) => panic!("unexpected {n} bytes after manager drop"),
        Err(_) => panic!("in-flight handshake survived manager drop"),
    }
}

#[tokio::test]
async fn stalled_handshake_is_reaped_for_next_server() {
    let (addr, mut manager) = manager(Some(ManagerConfig {
        establish_timeout: Duration::from_millis(300),
        max_concurrent_handshakes: 1,
        ..ManagerConfig::default()
    }))
    .await;
    let stalled = TcpStream::connect(addr).await.expect("raw TCP connect");
    let second = MockServer::dial(addr, "Second", vec![Activity::Playback], vec![])
        .await
        .unwrap();
    assert_eq!(
        next_connection(&mut manager).await.session.server_name,
        "Second"
    );
    drop(stalled);
    drop(second);
}
