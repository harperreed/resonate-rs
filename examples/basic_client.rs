// ABOUTME: Basic example demonstrating WebSocket connection and handshake
// ABOUTME: Connects to server, sends client/hello, receives server/hello

use resonate::protocol::client::ProtocolClient;
use resonate::protocol::messages::{AudioFormatSpec, ClientHello, DeviceInfo, PlayerSupport};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let hello = ClientHello {
        client_id: uuid::Uuid::new_v4().to_string(),
        name: "Resonate-RS Basic Client".to_string(),
        version: 1,
        supported_roles: vec!["player".to_string()],
        device_info: DeviceInfo {
            product_name: "Resonate-RS Player".to_string(),
            manufacturer: "Resonate".to_string(),
            software_version: "0.1.0".to_string(),
        },
        player_support: Some(PlayerSupport {
            support_formats: vec![AudioFormatSpec {
                codec: "pcm".to_string(),
                channels: 2,
                sample_rate: 48000,
                bit_depth: 24,
            }],
            buffer_capacity: 100,
            supported_commands: vec!["play".to_string(), "pause".to_string()],
        }),
        metadata_support: None,
    };

    println!("Connecting to ws://localhost:8927/resonate...");

    let _client = ProtocolClient::connect("ws://localhost:8927/resonate", hello).await?;

    println!("Connected! Waiting for server hello...");

    // This would block waiting for messages
    // For now, just demonstrate connection works

    Ok(())
}
