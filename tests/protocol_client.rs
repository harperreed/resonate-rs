mod common;

use common::MockServer;
use sendspin::error::Error;
use sendspin::protocol::messages::{
    Activity, ArtworkFormatRequest, ClientGoodbye, ClientState, ControllerCommandType,
    GoodbyeReason, GroupUpdate, Message, PlaybackState, PlayerFormatRequest, PlayerState,
    RepeatMode, ServerActivate, StreamEnd, StreamPlayerConfig, StreamStart,
};
use sendspin::ProtocolClientBuilder;
use tokio::net::TcpListener;
use tokio::time::{timeout, Duration};

async fn connected(
    builder: sendspin::ProtocolClientBuilder,
    roles: Vec<&str>,
) -> (sendspin::Connection, MockServer) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let roles: Vec<String> = roles.into_iter().map(str::to_string).collect();
    let server_task = tokio::spawn(async move {
        MockServer::accept(listener, "Test Server", vec![Activity::Playback], roles).await
    });
    let client = builder.connect(format!("ws://{addr}")).await.unwrap();
    let server = server_task.await.unwrap().unwrap();
    (client.split(), server)
}

async fn next_server_message(server: &mut MockServer) -> Message {
    timeout(Duration::from_secs(2), server.recv_json())
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn connect_completes_noise_handshake_and_exposes_session() {
    let (connection, server) = connected(
        ProtocolClientBuilder::builder()
            .name("Integration Client".into())
            .build(),
        vec!["player@v1"],
    )
    .await;
    assert_eq!(connection.session.server_name, "Test Server");
    assert_eq!(
        connection.session.initial_activities,
        vec![Activity::Playback]
    );
    assert_eq!(connection.session.initial_active_roles, vec!["player@v1"]);
    assert_eq!(
        connection.session.trust_level,
        sendspin::protocol::messages::TrustLevel::None
    );
    assert_eq!(connection.session.server_id.len(), 43);
    drop(connection);
    drop(server);
}

#[tokio::test]
async fn initial_state_and_external_source_transitions_are_serialized() {
    let player = PlayerState {
        volume: Some(42),
        ..Default::default()
    };
    let (connection, mut server) = connected(
        ProtocolClientBuilder::builder()
            .name("State Client".into())
            .initial_available(false)
            .initial_player_state(player.clone())
            .build(),
        vec!["player@v1"],
    )
    .await;
    assert!(matches!(
        next_server_message(&mut server).await,
        Message::ClientState(ClientState {
            available: false,
            player: Some(_),
            source: None
        })
    ));
    connection.enter_external_source().await.unwrap();
    assert!(matches!(
        next_server_message(&mut server).await,
        Message::ClientState(ClientState {
            available: false,
            player: None,
            source: None
        })
    ));
    connection
        .exit_external_source(Some(player.clone()))
        .await
        .unwrap();
    assert!(matches!(
        next_server_message(&mut server).await,
        Message::ClientState(ClientState {
            available: true,
            player: Some(_),
            source: None
        })
    ));
    drop(connection);
}

#[tokio::test]
async fn disconnect_sends_goodbye_and_closes_socket() {
    let (connection, mut server) = connected(
        ProtocolClientBuilder::builder()
            .name("Disconnecting".into())
            .build(),
        vec!["player@v1"],
    )
    .await;
    let _ = next_server_message(&mut server).await;
    timeout(
        Duration::from_secs(2),
        connection.guard.disconnect(GoodbyeReason::UserRequest),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(timeout(Duration::from_secs(2), server.recv_json())
        .await
        .unwrap()
        .unwrap()
        .matches_goodbye(GoodbyeReason::UserRequest));
    assert!(timeout(Duration::from_secs(2), server.recv_closed())
        .await
        .unwrap());
}

#[tokio::test]
async fn stream_format_requests_are_gated_by_active_streams() {
    let (mut connection, mut server) = connected(
        ProtocolClientBuilder::builder()
            .name("Formats".into())
            .build(),
        vec!["player@v1"],
    )
    .await;
    let _ = next_server_message(&mut server).await;
    let request = PlayerFormatRequest {
        codec: Some("pcm".into()),
        channels: None,
        sample_rate: None,
        bit_depth: None,
    };
    assert!(matches!(
        connection.sender.request_stream_format(None, None).await,
        Err(Error::Protocol(_))
    ));
    assert!(matches!(
        connection
            .sender
            .request_stream_format(Some(request.clone()), None)
            .await,
        Err(Error::Protocol(_))
    ));
    assert!(matches!(
        connection
            .sender
            .request_stream_format(
                None,
                Some(ArtworkFormatRequest {
                    channel: 0,
                    source: None,
                    format: None,
                    media_width: None,
                    media_height: None
                })
            )
            .await,
        Err(Error::Protocol(_))
    ));
    server
        .send_json(Message::StreamStart(StreamStart {
            server_transmitted: 1,
            player: Some(StreamPlayerConfig {
                codec: "pcm".into(),
                sample_rate: 48000,
                channels: 2,
                bit_depth: 16,
                codec_header: None,
            }),
            artwork: None,
            visualizer: None,
        }))
        .await
        .unwrap();
    assert!(matches!(
        timeout(Duration::from_secs(2), connection.messages.recv())
            .await
            .unwrap()
            .unwrap(),
        Message::StreamStart(_)
    ));
    connection
        .sender
        .request_stream_format(Some(request), None)
        .await
        .unwrap();
    assert!(matches!(
        next_server_message(&mut server).await,
        Message::StreamRequestFormat(_)
    ));
    server
        .send_json(Message::StreamEnd(StreamEnd {
            server_transmitted: 2,
            roles: None,
        }))
        .await
        .unwrap();
    assert!(matches!(
        timeout(Duration::from_secs(2), connection.messages.recv())
            .await
            .unwrap()
            .unwrap(),
        Message::StreamEnd(_)
    ));
    assert!(matches!(
        connection
            .sender
            .request_stream_format(
                Some(PlayerFormatRequest {
                    codec: None,
                    channels: Some(1),
                    sample_rate: None,
                    bit_depth: None
                }),
                None
            )
            .await,
        Err(Error::Protocol(_))
    ));
    drop(connection);
}

#[tokio::test]
async fn controller_commands_wire_and_live_activation_updates_roles() {
    let (connection, mut server) = connected(
        ProtocolClientBuilder::builder()
            .name("Controller".into())
            .controller()
            .build(),
        vec!["player@v1", "controller@v1"],
    )
    .await;
    let _ = next_server_message(&mut server).await;
    let controller = connection.controller.clone().unwrap();
    controller.play().await.unwrap();
    assert!(
        matches!(next_server_message(&mut server).await, Message::ClientCommand(c) if c.controller.clone().unwrap().command == ControllerCommandType::Play)
    );
    controller.pause().await.unwrap();
    assert!(
        matches!(next_server_message(&mut server).await, Message::ClientCommand(c) if c.controller.clone().unwrap().command == ControllerCommandType::Pause)
    );
    controller.set_volume(150).await.unwrap();
    assert!(
        matches!(next_server_message(&mut server).await, Message::ClientCommand(c) if c.controller.clone().unwrap().volume == Some(100))
    );
    controller.set_mute(true).await.unwrap();
    assert!(
        matches!(next_server_message(&mut server).await, Message::ClientCommand(c) if c.controller.clone().unwrap().mute == Some(true))
    );
    controller.repeat(RepeatMode::All).await.unwrap();
    assert!(
        matches!(next_server_message(&mut server).await, Message::ClientCommand(c) if c.controller.clone().unwrap().command == ControllerCommandType::RepeatAll)
    );
    controller.shuffle(false).await.unwrap();
    assert!(
        matches!(next_server_message(&mut server).await, Message::ClientCommand(c) if c.controller.clone().unwrap().command == ControllerCommandType::Unshuffle)
    );
    controller.seek(1234).await.unwrap();
    assert!(matches!(
        next_server_message(&mut server).await,
        Message::ClientCommand(c) if c.controller.clone().unwrap().position_ms == Some(1234)
    ));
    controller.seek_relative(-55).await.unwrap();
    assert!(matches!(
        next_server_message(&mut server).await,
        Message::ClientCommand(c) if c.controller.clone().unwrap().offset_ms == Some(-55)
    ));
    drop(connection);
}

#[tokio::test]
async fn controller_denial_and_later_live_role_activation() {
    let (mut connection, mut server) = connected(
        ProtocolClientBuilder::builder()
            .name("Controller".into())
            .controller()
            .build(),
        vec!["player@v1"],
    )
    .await;
    let _ = next_server_message(&mut server).await;
    let controller = connection.controller.clone().unwrap();
    assert!(matches!(controller.play().await, Err(Error::Protocol(_))));
    server
        .send_json(Message::ServerActivate(ServerActivate {
            activities: vec![Activity::Playback],
            active_roles: Some(vec!["player@v1".into(), "controller@v1".into()]),
            pairing: None,
        }))
        .await
        .unwrap();
    assert!(matches!(
        timeout(Duration::from_secs(2), connection.messages.recv())
            .await
            .unwrap()
            .unwrap(),
        Message::ServerActivate(_)
    ));
    assert!(connection
        .active_roles()
        .iter()
        .any(|r| r == "controller@v1"));
    assert!(matches!(
        next_server_message(&mut server).await,
        Message::ClientState(_)
    ));
    controller.play().await.unwrap();
    assert!(matches!(
        next_server_message(&mut server).await,
        Message::ClientCommand(_)
    ));
    drop(connection);

    let (connection, _server) = connected(
        ProtocolClientBuilder::builder()
            .name("No Controller".into())
            .build(),
        vec!["player@v1", "controller@v1"],
    )
    .await;
    assert!(connection.controller.is_none());
    drop(connection);
}

#[tokio::test]
async fn encrypted_audio_json_forward_and_server_time_is_consumed() {
    let (mut connection, server) = connected(
        ProtocolClientBuilder::builder()
            .name("Receiver".into())
            .build(),
        vec!["player@v1"],
    )
    .await;
    server.send_audio(1234, &[1, 2, 3, 4]).await.unwrap();
    let audio = timeout(Duration::from_secs(2), connection.audio.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(audio.timestamp, 1234);
    assert_eq!(&*audio.data, &[1, 2, 3, 4]);
    server
        .send_json(Message::GroupUpdate(GroupUpdate {
            playback_state: Some(PlaybackState::Playing),
            group_id: Some("g".into()),
            group_name: None,
        }))
        .await
        .unwrap();
    assert!(matches!(
        timeout(Duration::from_secs(2), connection.messages.recv())
            .await
            .unwrap()
            .unwrap(),
        Message::GroupUpdate(_)
    ));
    server
        .send_json(Message::ServerTime(
            sendspin::protocol::messages::ServerTime {
                client_transmitted: 1,
                server_received: 2,
                server_transmitted: 3,
            },
        ))
        .await
        .unwrap();
    assert!(
        timeout(Duration::from_millis(100), connection.messages.recv())
            .await
            .is_err()
    );
    drop(connection);
}

trait GoodbyeMatch {
    fn matches_goodbye(self, reason: GoodbyeReason) -> bool;
}
impl GoodbyeMatch for Message {
    fn matches_goodbye(self, reason: GoodbyeReason) -> bool {
        matches!(self, Message::ClientGoodbye(ClientGoodbye { reason: actual }) if actual == reason)
    }
}
