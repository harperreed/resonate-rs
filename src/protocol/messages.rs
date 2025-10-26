// ABOUTME: Protocol message type definitions and serialization
// ABOUTME: Supports client/hello, server/hello, stream/start, etc.

use serde::{Deserialize, Serialize};

/// Top-level protocol message envelope
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum Message {
    #[serde(rename = "client/hello")]
    ClientHello(ClientHello),

    #[serde(rename = "server/hello")]
    ServerHello(ServerHello),

    #[serde(rename = "client/time")]
    ClientTime(ClientTime),

    #[serde(rename = "server/time")]
    ServerTime(ServerTime),

    #[serde(rename = "stream/start")]
    StreamStart(StreamStart),

    #[serde(rename = "server/command")]
    ServerCommand(ServerCommand),

    #[serde(rename = "player/update")]
    PlayerUpdate(PlayerUpdate),
}

/// Client hello message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientHello {
    pub client_id: String,
    pub name: String,
    pub version: u32,
    pub supported_roles: Vec<String>,
    pub device_info: DeviceInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub player_support: Option<PlayerSupport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata_support: Option<MetadataSupport>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub product_name: String,
    pub manufacturer: String,
    pub software_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerSupport {
    pub support_formats: Vec<AudioFormatSpec>,
    pub buffer_capacity: u32,
    pub supported_commands: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioFormatSpec {
    pub codec: String,
    pub channels: u8,
    pub sample_rate: u32,
    pub bit_depth: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataSupport {
    pub support_picture_formats: Vec<String>,
    pub media_width: u32,
    pub media_height: u32,
}

/// Server hello message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerHello {
    pub server_id: String,
    pub name: String,
    pub version: u32,
}

/// Client time sync message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientTime {
    pub client_transmitted: i64,
}

/// Server time sync response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerTime {
    pub client_transmitted: i64,
    pub server_received: i64,
    pub server_transmitted: i64,
}

/// Stream start message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamStart {
    pub player: StreamPlayerConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamPlayerConfig {
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u8,
    pub bit_depth: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec_header: Option<String>,
}

/// Server command message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerCommand {
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mute: Option<bool>,
}

/// Player state update message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerUpdate {
    pub state: String,
    pub volume: u8,
    pub muted: bool,
}
