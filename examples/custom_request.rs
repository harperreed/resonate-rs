// ABOUTME: Example demonstrating builder capabilities with a custom WebSocket request
// ABOUTME: Shows auth-proxy headers, persisted credentials, player format config, and controller role
// Run with: cargo run --example custom_request

use clap::Parser;
use sendspin::protocol::messages::{
    AudioFormatSpec, PlayerState, PlayerStateCommand, PlayerV1Support,
};
use sendspin::{ClientCredentials, ProtocolClientBuilder};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

/// Sendspin advanced client
#[derive(Parser, Debug)]
#[command(name = "custom_request")]
#[command(about = "Demonstrates builder capabilities with custom headers", long_about = None)]
struct Args {
    /// WebSocket URL of the Sendspin server. Per spec the transport is plain
    /// ws:// — confidentiality comes from the mandatory Noise layer, not TLS.
    #[arg(short, long, default_value = "ws://localhost:8927/sendspin")]
    server: String,

    /// Client name
    #[arg(short, long, default_value = "Sendspin-RS Advanced Client")]
    name: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let args = Args::parse();

    println!("Connecting to {}...", args.server);

    // Build a request with custom headers (e.g., for an HTTP auth proxy in
    // front of the server). Anything implementing IntoClientRequest works;
    // apps that need to own the transport entirely (custom TLS, HTTP
    // routing) can instead hand a ready WebSocketStream to
    // `ProtocolClientBuilder::accept`.
    let mut request = args.server.into_client_request()?;
    request
        .headers_mut()
        .insert("cookie", "ingress_session=<session_token>".parse()?);

    // Persist credentials.to_bytes() in application-owned secure storage so
    // servers recognize this client across restarts. Restore with
    // ClientCredentials::from_bytes() on the next launch.
    let credentials = ClientCredentials::generate()?;

    // Configure the builder with explicit player support and controller role
    let client = ProtocolClientBuilder::builder()
        .name(args.name)
        .credentials(credentials)
        .product_name(Some("Sendspin-RS Advanced Client".to_string()))
        .software_version(Some(env!("CARGO_PKG_VERSION").to_string()))
        // Declare player capabilities: 24-bit/48kHz stereo PCM
        .player_v1_support(PlayerV1Support {
            supported_formats: vec![AudioFormatSpec {
                codec: "pcm".to_string(),
                channels: 2,
                sample_rate: 48000,
                bit_depth: 24,
            }],
            buffer_capacity: 50 * 1024 * 1024,
        })
        // Request controller role for playback control
        .controller()
        // Set initial player state
        .initial_player_state(PlayerState {
            volume: Some(80),
            muted: Some(false),
            output_delay_ms: 0,
            required_lead_time_ms: 500,
            min_buffer_ms: 500,
            supported_commands: vec![PlayerStateCommand::Volume, PlayerStateCommand::Mute],
            format: None,
        })
        .build()
        .connect(request)
        .await?;

    println!("Connected!");

    // Split into conn channels for concurrent use
    let conn = client.split();

    match conn.controller {
        Some(controller) if conn.active_roles().iter().any(|r| r == "controller@v1") => {
            println!("Controller role active — can send playback commands");
            // e.g., controller.play().await?;
            drop(controller);
        }
        Some(_) => println!("Controller declared but not activated by the server"),
        None => println!("Controller role not declared"),
    }

    Ok(())
}
