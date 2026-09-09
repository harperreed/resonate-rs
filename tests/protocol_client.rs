mod common;

use common::{test_credentials, MockServer};
use sendspin::error::Error;
use sendspin::protocol::crypto::{Identity, Psk};
use sendspin::protocol::messages::{
    ActivatePairing, Activity, ArtworkChannelConfig, ArtworkSource, ArtworkState, AudioFormatSpec,
    ClientGoodbye, ClientState, ControllerCommandType, GoodbyeReason, GroupUpdate, ImageFormat,
    Message, PairAbortReason, PairingMethod, PlaybackState, PlayerState, PlayerV1Support,
    RepeatMode, ServerActivate, ServerUnpair, SourceSignal, SourceState, VisualizerDataType,
    VisualizerState,
};
use sendspin::ProtocolClientBuilder;
use tokio::net::TcpListener;
use tokio::time::{timeout, Duration};

fn source_stream_config() -> sendspin::protocol::messages::SourceStreamConfig {
    sendspin::protocol::messages::SourceStreamConfig {
        codec: "pcm".into(),
        channels: 2,
        sample_rate: 48_000,
        bit_depth: 16,
        codec_header: None,
    }
}

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
async fn initial_pairing_method_mismatch_sends_abort_without_finalize() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_task = tokio::spawn(async move {
        common::MockServer::accept_with_pairing(
            listener,
            "Pairing Server",
            vec![Activity::Pairing],
            vec![],
            Some(ActivatePairing {
                method: PairingMethod::StaticPairingCode,
                format: None,
            }),
        )
        .await
    });
    let client = ProtocolClientBuilder::builder()
        .credentials(test_credentials())
        .name("Pairing Client".into())
        .build()
        .connect(format!("ws://{addr}"))
        .await
        .unwrap();
    let mut server = server_task.await.unwrap().unwrap();
    assert!(matches!(
        next_server_message(&mut server).await,
        Message::PairAbort(sendspin::protocol::messages::PairAbort {
            reason: PairAbortReason::MethodNotSupported
        })
    ));
    assert!(timeout(Duration::from_millis(100), server.recv_json())
        .await
        .is_err());
    drop(client);
}

#[tokio::test]
async fn connect_completes_noise_handshake_and_exposes_session() {
    let (connection, server) = connected(
        ProtocolClientBuilder::builder()
            .credentials(test_credentials())
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
    assert!(!connection.session.paired);
    assert_eq!(connection.session.server_id.len(), 43);
    drop(connection);
    drop(server);
}

#[tokio::test]
async fn initial_player_state_waits_for_clock_sync() {
    let (connection, mut server) = connected(
        ProtocolClientBuilder::builder()
            .credentials(test_credentials())
            .name("Sync-gated Client".into())
            .build(),
        vec!["player@v1"],
    )
    .await;

    assert!(matches!(
        next_server_message(&mut server).await,
        Message::ClientState(ClientState {
            available: false,
            player: Some(_),
            ..
        })
    ));
    assert!(timeout(Duration::from_millis(5), server.recv_json())
        .await
        .is_err());

    let synced = timeout(Duration::from_secs(2), async {
        loop {
            if matches!(server.recv_json().await.unwrap(), Message::ClientState(state) if state.available)
            {
                break;
            }
        }
    })
    .await;
    assert!(
        synced.is_ok(),
        "available=true was not sent after clock sync"
    );
    drop(connection);
}

#[tokio::test]
async fn initial_source_state_waits_for_clock_sync() {
    let (connection, mut server) = connected(
        ProtocolClientBuilder::builder()
            .credentials(test_credentials())
            .name("Sync-gated Source".into())
            .source_v1_support(Default::default())
            .build(),
        vec!["source@v1"],
    )
    .await;

    assert!(matches!(
        next_server_message(&mut server).await,
        Message::ClientState(ClientState {
            available: false,
            source: None,
            ..
        })
    ));
    let synced = timeout(Duration::from_secs(2), async {
        loop {
            if matches!(server.recv_json().await.unwrap(), Message::ClientState(state) if state.available)
            {
                break;
            }
        }
    })
    .await;
    assert!(
        synced.is_ok(),
        "available=true was not sent after clock sync"
    );
    drop(connection);
}

#[tokio::test]
async fn source_state_updates_are_full_state_messages() {
    let (connection, mut server) = connected(
        ProtocolClientBuilder::builder()
            .credentials(test_credentials())
            .name("Source state".into())
            .source_v1_support(Default::default())
            .initial_available(false)
            .build(),
        vec!["source@v1"],
    )
    .await;
    let _ = next_server_message(&mut server).await;
    connection
        .sender
        .update_source_state(SourceState {
            signal: Some(SourceSignal::Present),
        })
        .await
        .unwrap();
    assert!(matches!(
        next_server_message(&mut server).await,
        Message::ClientState(ClientState {
            available: false,
            source: Some(SourceState {
                signal: Some(SourceSignal::Present)
            }),
            ..
        })
    ));
    drop(connection);
}

#[tokio::test]
async fn initial_state_and_external_source_transitions_are_serialized() {
    let player = PlayerState {
        volume: Some(42),
        ..Default::default()
    };
    let (connection, mut server) = connected(
        ProtocolClientBuilder::builder()
            .credentials(test_credentials())
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
            source: None,
            artwork: None,
            visualizer: None
        })
    ));
    connection.enter_external_source().await.unwrap();
    assert!(matches!(
        next_server_message(&mut server).await,
        Message::ClientState(ClientState {
            available: false,
            player: None,
            source: None,
            artwork: None,
            visualizer: None
        })
    ));
    connection
        .exit_external_source(Some(player.clone()))
        .await
        .unwrap();
    let became_available = timeout(Duration::from_secs(2), async {
        loop {
            if matches!(
                next_server_message(&mut server).await,
                Message::ClientState(ClientState {
                    available: true,
                    player: Some(_),
                    source: None,
                    artwork: None,
                    visualizer: None
                })
            ) {
                break;
            }
        }
    })
    .await;
    assert!(became_available.is_ok());
    drop(connection);
}

#[tokio::test]
async fn disconnect_sends_goodbye_and_closes_socket() {
    let (connection, mut server) = connected(
        ProtocolClientBuilder::builder()
            .credentials(test_credentials())
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
async fn initial_state_includes_player_artwork_and_visualizer_objects() {
    let (connection, mut server) = connected(
        ProtocolClientBuilder::builder()
            .credentials(test_credentials())
            .name("State objects".into())
            .player_v1_support(PlayerV1Support {
                supported_formats: vec![AudioFormatSpec {
                    codec: "pcm".into(),
                    channels: 2,
                    sample_rate: 48_000,
                    bit_depth: 16,
                }],
                buffer_capacity: 1024,
            })
            .artwork_state(ArtworkState {
                channels: vec![ArtworkChannelConfig {
                    source: ArtworkSource::Album,
                    format: Some(ImageFormat::Png),
                    width: Some(64),
                    height: Some(64),
                }],
            })
            .visualizer_v1_support(sendspin::protocol::messages::VisualizerV1Support {
                buffer_capacity: 100,
            })
            .visualizer_state(VisualizerState {
                types: vec![VisualizerDataType::Loudness],
                rate_max: 10,
                spectrum: None,
            })
            .build(),
        vec!["player@v1", "artwork@v1", "visualizer@v1"],
    )
    .await;
    match next_server_message(&mut server).await {
        Message::ClientState(ClientState {
            player: Some(player),
            artwork: Some(artwork),
            visualizer: Some(visualizer),
            ..
        }) => {
            assert_eq!(player.volume, Some(100));
            assert_eq!(artwork.channels.len(), 1);
            assert_eq!(visualizer.types, vec![VisualizerDataType::Loudness]);
        }
        other => panic!("expected complete initial state, got {other:?}"),
    }
    drop(connection);
}

#[tokio::test]
async fn invalid_runtime_player_state_is_rejected_without_sending() {
    let (connection, mut server) = connected(
        ProtocolClientBuilder::builder()
            .credentials(test_credentials())
            .name("Validation".into())
            .initial_available(false)
            .build(),
        vec!["player@v1"],
    )
    .await;
    let _ = next_server_message(&mut server).await;
    let invalid = PlayerState {
        output_delay_ms: 5001,
        ..Default::default()
    };
    assert!(matches!(
        connection.sender.update_player_state(invalid).await,
        Err(Error::Protocol(message)) if message.contains("output_delay_ms")
    ));
    assert!(timeout(Duration::from_millis(50), server.recv_json())
        .await
        .is_err());
}

#[tokio::test]
async fn source_end_stream_is_allowed_after_role_removal() {
    let (mut connection, mut server) = connected(
        ProtocolClientBuilder::builder()
            .credentials(test_credentials())
            .name("Source cleanup".into())
            .source_v1_support(Default::default())
            .initial_available(false)
            .build(),
        vec!["source@v1"],
    )
    .await;
    let _ = next_server_message(&mut server).await;
    let source = connection.source.as_ref().unwrap();
    source.start_stream(source_stream_config()).await.unwrap();
    assert!(matches!(
        next_server_message(&mut server).await,
        Message::ClientStreamStart(_)
    ));
    source.send_chunk(123, &[1, 2, 3]).await.unwrap();
    source.end_stream().await.unwrap();
    assert!(matches!(
        next_server_message(&mut server).await,
        Message::ClientStreamEnd(_)
    ));
    server
        .send_json(Message::ServerActivate(ServerActivate {
            activities: vec![Activity::Playback],
            active_roles: Some(Vec::new()),
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
    assert!(matches!(
        source.start_stream(source_stream_config()).await,
        Err(Error::Protocol(_))
    ));
    assert!(matches!(
        source.send_chunk(456, &[4, 5, 6]).await,
        Err(Error::Protocol(_))
    ));
    source.end_stream().await.unwrap();
    assert!(matches!(
        next_server_message(&mut server).await,
        Message::ClientStreamEnd(_)
    ));
    server
        .send_json(Message::ServerActivate(ServerActivate {
            activities: vec![Activity::Playback],
            active_roles: Some(Vec::new()),
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
    assert!(matches!(
        source.start_stream(source_stream_config()).await,
        Err(Error::Protocol(_))
    ));
    assert!(matches!(
        source.send_chunk(456, &[4, 5, 6]).await,
        Err(Error::Protocol(_))
    ));
    source.end_stream().await.unwrap();
    assert!(matches!(
        next_server_message(&mut server).await,
        Message::ClientStreamEnd(_)
    ));
    drop(connection);
}

#[tokio::test]
async fn server_unpair_sends_unpaired_goodbye_for_paired_session() {
    let pairing_psk = Psk::new([7u8; 32]);
    let server_identity = Identity::generate().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_task = tokio::spawn({
        let server_identity = server_identity.clone();
        let pairing_psk = pairing_psk.clone();
        async move {
            common::MockServer::accept_with_long_term_psk(
                listener,
                "Paired Server",
                vec![Activity::Playback],
                vec!["player@v1".into()],
                server_identity,
                pairing_psk,
            )
            .await
        }
    });
    let store = std::sync::Arc::new(
        sendspin::protocol::pairing::MemoryPairingStore::with_records(vec![
            sendspin::protocol::pairing::PairingRecord {
                psk: pairing_psk,
                server_id: server_identity.id(),
                used: false,
            },
        ]),
    );
    let client = ProtocolClientBuilder::builder()
        .credentials(test_credentials())
        .name("Paired Client".into())
        .pairing_store(store)
        .build()
        .connect(format!("ws://{addr}"))
        .await
        .unwrap();
    let mut server = server_task.await.unwrap().unwrap();
    let _ = next_server_message(&mut server).await;
    server
        .send_json(Message::ServerUnpair(ServerUnpair {}))
        .await
        .unwrap();
    assert!(matches!(
        next_server_message(&mut server).await,
        Message::ClientGoodbye(ClientGoodbye {
            reason: GoodbyeReason::Unpaired
        })
    ));
    assert!(timeout(Duration::from_secs(2), server.recv_closed())
        .await
        .unwrap());
    drop(client);
}

#[tokio::test]
async fn player_state_updates_are_full_state_messages() {
    let (connection, mut server) = connected(
        ProtocolClientBuilder::builder()
            .credentials(test_credentials())
            .name("Formats".into())
            .build(),
        vec!["player@v1"],
    )
    .await;
    let initial = next_server_message(&mut server).await;
    assert!(matches!(
        initial,
        Message::ClientState(ClientState {
            player: Some(_),
            ..
        })
    ));
    connection
        .sender
        .set_player_format(Some(AudioFormatSpec {
            codec: "pcm".into(),
            channels: 2,
            sample_rate: 48_000,
            bit_depth: 16,
        }))
        .await
        .unwrap();
    assert!(matches!(
        next_server_message(&mut server).await,
        Message::ClientState(ClientState {
            player: Some(_),
            ..
        })
    ));
}

#[tokio::test]
async fn controller_commands_wire_and_live_activation_updates_roles() {
    let (connection, mut server) = connected(
        ProtocolClientBuilder::builder()
            .credentials(test_credentials())
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
            .credentials(test_credentials())
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
            .credentials(test_credentials())
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
            .credentials(test_credentials())
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
            playback_state: PlaybackState::Playing,
            group_id: "g".into(),
            group_name: "Group".into(),
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
