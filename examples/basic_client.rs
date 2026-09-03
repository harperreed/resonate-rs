// ABOUTME: Basic example demonstrating WebSocket connection and handshake
// ABOUTME: Connects to server, sends client/hello, receives server/hello

use clap::Parser;
use sendspin::{ClientCredentials, ProtocolClientBuilder};

/// Sendspin basic client
#[derive(Parser, Debug)]
#[command(name = "basic_client")]
#[command(about = "Test connection to Sendspin server", long_about = None)]
struct Args {
    /// WebSocket URL of the Sendspin server
    #[arg(short, long, default_value = "ws://localhost:8927/sendspin")]
    server: String,

    /// Client name
    #[arg(short, long, default_value = "Sendspin-RS Basic Client")]
    name: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let args = Args::parse();

    println!("Connecting to {}...", args.server);

    // Persist credentials.to_bytes() in application-owned secure storage and
    // restore them with ClientCredentials::from_bytes() on the next launch.
    let credentials = ClientCredentials::generate()?;
    let test = ProtocolClientBuilder::builder()
        .credentials(credentials)
        .name(args.name.clone())
        .build();

    let _client = test.connect(&args.server).await?;

    println!("Connected! Waiting for server hello...");

    // This would block waiting for messages
    // For now, just demonstrate connection works

    Ok(())
}
