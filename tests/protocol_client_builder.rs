mod common;

use common::{test_credentials, MockServer};
use sendspin::error::Error;
use sendspin::protocol::messages::{
    ArtworkChannelConfig, ArtworkSource, ArtworkState, AudioFormatSpec, ImageFormat, PlayerState,
    PlayerStateCommand, PlayerV1Support, SourceV1Support, VisualizerDataType, VisualizerState,
    VisualizerV1Support,
};
use sendspin::ProtocolClientBuilder;
use tokio::net::TcpListener;

fn player() -> PlayerV1Support {
    PlayerV1Support {
        supported_formats: vec![AudioFormatSpec {
            codec: "pcm".into(),
            channels: 2,
            sample_rate: 48_000,
            bit_depth: 16,
        }],
        buffer_capacity: 1024,
    }
}

#[test]
fn default_builder_has_player_role() {
    let b = ProtocolClientBuilder::builder()
        .credentials(test_credentials())
        .name("Test".into())
        .build();
    assert_eq!(b.supported_roles(), &["player@v1"]);
    assert!(b.player_v1_support().is_some());
}

#[tokio::test]
async fn zero_artwork_transfer_limit_is_rejected_before_handshake() {
    let builder = ProtocolClientBuilder::builder()
        .credentials(test_credentials())
        .name("Test".into())
        .max_encoded_artwork_transfer_bytes(0)
        .build();
    assert!(matches!(
        builder.connect("ws://127.0.0.1:1").await,
        Err(Error::Protocol(message))
            if message.contains("max_encoded_artwork_transfer_bytes")
    ));
}

#[tokio::test]
async fn initial_player_format_must_be_supported() {
    let builder = ProtocolClientBuilder::builder()
        .credentials(test_credentials())
        .name("Test".into())
        .initial_player_state(PlayerState {
            format: Some(AudioFormatSpec {
                codec: "flac".into(),
                channels: 2,
                sample_rate: 48_000,
                bit_depth: 24,
            }),
            ..Default::default()
        })
        .build();
    assert!(matches!(
        builder.connect("ws://127.0.0.1:1").await,
        Err(Error::Protocol(message)) if message.contains("supported_formats")
    ));
}

#[tokio::test]
async fn initial_role_state_requires_declared_role() {
    let builder = ProtocolClientBuilder::builder()
        .credentials(test_credentials())
        .name("Test".into())
        .metadata()
        .initial_player_state(PlayerState::default())
        .build();
    assert!(matches!(
        builder.connect("ws://127.0.0.1:1").await,
        Err(Error::Protocol(message)) if message.contains("player_v1_support")
    ));
}

#[test]
fn explicit_roles_are_composed() {
    let b = ProtocolClientBuilder::builder()
        .credentials(test_credentials())
        .name("Test".into())
        .player_v1_support(player())
        .metadata()
        .controller()
        .color()
        .build();
    assert_eq!(
        b.supported_roles(),
        &["player@v1", "metadata@v1", "controller@v1", "color@v1"]
    );
}

#[test]
fn roles_without_player_are_supported() {
    let b = ProtocolClientBuilder::builder()
        .credentials(test_credentials())
        .name("Test".into())
        .metadata()
        .build();
    assert_eq!(b.supported_roles(), &["metadata@v1"]);
    assert!(b.player_v1_support().is_none());
}

#[tokio::test]
async fn invalid_initial_player_command_state_is_rejected() {
    let builder = ProtocolClientBuilder::builder()
        .credentials(test_credentials())
        .name("Test".into())
        .initial_player_state(PlayerState {
            supported_commands: vec![PlayerStateCommand::Volume, PlayerStateCommand::Mute],
            ..Default::default()
        })
        .build();
    let result = builder.connect("ws://127.0.0.1:1").await;
    assert!(matches!(result, Err(Error::Protocol(_))));
}

#[tokio::test]
async fn read_only_initial_player_volume_and_mute_are_valid() {
    let builder = ProtocolClientBuilder::builder()
        .credentials(test_credentials())
        .name("Test".into())
        .initial_player_state(PlayerState {
            volume: Some(42),
            muted: Some(false),
            ..Default::default()
        })
        .build();
    let result = builder.connect("ws://127.0.0.1:1").await;
    assert!(!matches!(result, Err(Error::Protocol(_))));
}

#[test]
fn identity_and_crypto_options_are_accepted() {
    let credentials = sendspin::ClientCredentials::generate().unwrap();
    let b = ProtocolClientBuilder::builder()
        .name("Test".into())
        .credentials(credentials)
        .suite(sendspin::CipherSuite::ChaChaPoly)
        .unpaired_access(false)
        .build();
    assert_eq!(b.supported_roles(), &["player@v1"]);
}

#[tokio::test]
async fn declared_role_support_objects_reach_encrypted_client_hello() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_task = tokio::spawn(async move {
        MockServer::accept(
            listener,
            "Builder Server",
            vec![sendspin::protocol::messages::Activity::Playback],
            vec!["player@v1".into()],
        )
        .await
    });
    let client = ProtocolClientBuilder::builder()
        .credentials(test_credentials())
        .name("Builder Client".into())
        .controller()
        .source_v1_support(SourceV1Support::default())
        .artwork_state(ArtworkState {
            channels: vec![ArtworkChannelConfig {
                source: ArtworkSource::Album,
                format: Some(ImageFormat::Png),
                width: Some(320),
                height: Some(240),
            }],
        })
        .visualizer_v1_support(VisualizerV1Support {
            buffer_capacity: 4096,
        })
        .visualizer_state(VisualizerState {
            types: vec![VisualizerDataType::Loudness],
            rate_max: 30,
            spectrum: None,
        })
        .build()
        .connect(format!("ws://{addr}"))
        .await
        .unwrap();
    let server = server_task.await.unwrap().unwrap();
    assert!(server
        .client_hello
        .supported_roles
        .iter()
        .any(|r| r == "controller@v1"));
    assert!(server
        .client_hello
        .supported_roles
        .iter()
        .any(|r| r == "source@v1"));
    assert!(server
        .client_hello
        .supported_roles
        .iter()
        .any(|r| r == "artwork@v1"));
    assert!(server
        .client_hello
        .supported_roles
        .iter()
        .any(|r| r == "visualizer@v1"));
    assert!(server.client_hello.visualizer_v1_support.is_some());
    assert!(server.client_hello.source_v1_support.is_some());
    assert!(server
        .client_hello
        .supported_pair_methods
        .pairing_psk
        .is_some());
    drop(client);
}
