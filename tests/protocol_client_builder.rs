mod common;

use common::MockServer;
use sendspin::protocol::messages::{
    ArtworkChannel, ArtworkSource, AudioFormatSpec, ImageFormat, PlayerV1Support, SourceV1Support,
    VisualizerDataType, VisualizerV1Support,
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
        supported_commands: vec!["volume".into()],
    }
}

#[test]
fn default_builder_has_player_role() {
    let b = ProtocolClientBuilder::builder().name("Test".into()).build();
    assert_eq!(b.supported_roles(), &["player@v1"]);
    assert!(b.player_v1_support().is_some());
}

#[test]
fn explicit_roles_are_composed() {
    let b = ProtocolClientBuilder::builder()
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
        .name("Test".into())
        .metadata()
        .build();
    assert_eq!(b.supported_roles(), &["metadata@v1"]);
    assert!(b.player_v1_support().is_none());
}

#[test]
fn identity_and_crypto_options_are_accepted() {
    let identity = sendspin::Identity::generate().unwrap();
    let b = ProtocolClientBuilder::builder()
        .name("Test".into())
        .identity(identity)
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
        .name("Builder Client".into())
        .controller()
        .source_v1_support(SourceV1Support::default())
        .artwork_v1_support(sendspin::protocol::messages::ArtworkV1Support {
            channels: vec![ArtworkChannel {
                source: ArtworkSource::Album,
                format: ImageFormat::Png,
                media_width: 320,
                media_height: 240,
            }],
        })
        .visualizer_v1_support(VisualizerV1Support {
            types: vec![VisualizerDataType::Loudness],
            buffer_capacity: 4096,
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
    assert!(server.client_hello.artwork_v1_support.is_some());
    assert!(server.client_hello.visualizer_v1_support.is_some());
    assert!(server.client_hello.source_v1_support.is_some());
    drop(client);
}
