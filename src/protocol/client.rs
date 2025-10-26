// ABOUTME: WebSocket client implementation for Resonate protocol
// ABOUTME: Handles connection, message routing, and protocol state machine

use crate::error::Error;
use crate::protocol::messages::{ClientHello, Message};
use crate::sync::ClockSync;
use futures_util::{
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

/// Audio chunk from server (binary frame)
#[derive(Debug, Clone)]
pub struct AudioChunk {
    /// Server timestamp in microseconds
    pub timestamp: i64,
    /// Raw audio data bytes
    pub data: Arc<[u8]>,
}

impl AudioChunk {
    /// Parse from WebSocket binary frame
    pub fn from_bytes(frame: &[u8]) -> Result<Self, Error> {
        if frame.len() < 9 {
            return Err(Error::Protocol("Audio chunk too short".to_string()));
        }

        if frame[0] != 0x01 {
            return Err(Error::Protocol("Invalid audio chunk type".to_string()));
        }

        let timestamp = i64::from_be_bytes([
            frame[1], frame[2], frame[3], frame[4], frame[5], frame[6], frame[7], frame[8],
        ]);

        let data = Arc::from(&frame[9..]);

        Ok(Self { timestamp, data })
    }
}

/// WebSocket client for Resonate protocol
pub struct ProtocolClient {
    #[allow(dead_code)]
    ws_tx:
        Arc<tokio::sync::Mutex<SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, WsMessage>>>,
    audio_rx: UnboundedReceiver<AudioChunk>,
    message_rx: UnboundedReceiver<Message>,
    #[allow(dead_code)]
    clock_sync: Arc<tokio::sync::Mutex<ClockSync>>,
}

impl ProtocolClient {
    /// Connect to Resonate server
    pub async fn connect(url: &str, hello: ClientHello) -> Result<Self, Error> {
        // Connect WebSocket
        let (ws_stream, _) = connect_async(url)
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        let (mut write, read) = ws_stream.split();

        // Send client hello
        let hello_msg = Message::ClientHello(hello);
        let hello_json =
            serde_json::to_string(&hello_msg).map_err(|e| Error::Protocol(e.to_string()))?;

        write
            .send(WsMessage::Text(hello_json))
            .await
            .map_err(|e| Error::WebSocket(e.to_string()))?;

        // Wait for server hello
        let mut read_temp = read;
        if let Some(Ok(WsMessage::Text(text))) = read_temp.next().await {
            let msg: Message =
                serde_json::from_str(&text).map_err(|e| Error::Protocol(e.to_string()))?;

            match msg {
                Message::ServerHello(server_hello) => {
                    log::info!(
                        "Connected to server: {} ({})",
                        server_hello.name,
                        server_hello.server_id
                    );
                }
                _ => return Err(Error::Protocol("Expected server/hello".to_string())),
            }
        } else {
            return Err(Error::Connection("No server hello received".to_string()));
        }

        // Create channels for message routing
        let (audio_tx, audio_rx) = unbounded_channel();
        let (message_tx, message_rx) = unbounded_channel();

        let clock_sync = Arc::new(tokio::sync::Mutex::new(ClockSync::new()));

        // Spawn message router task
        let clock_sync_clone = Arc::clone(&clock_sync);
        tokio::spawn(async move {
            Self::message_router(read_temp, audio_tx, message_tx, clock_sync_clone).await;
        });

        Ok(Self {
            ws_tx: Arc::new(tokio::sync::Mutex::new(write)),
            audio_rx,
            message_rx,
            clock_sync,
        })
    }

    async fn message_router(
        mut read: SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
        audio_tx: UnboundedSender<AudioChunk>,
        message_tx: UnboundedSender<Message>,
        _clock_sync: Arc<tokio::sync::Mutex<ClockSync>>,
    ) {
        while let Some(msg) = read.next().await {
            match msg {
                Ok(WsMessage::Binary(data)) => {
                    if let Ok(chunk) = AudioChunk::from_bytes(&data) {
                        let _ = audio_tx.send(chunk);
                    }
                }
                Ok(WsMessage::Text(text)) => {
                    if let Ok(msg) = serde_json::from_str::<Message>(&text) {
                        let _ = message_tx.send(msg);
                    }
                }
                Ok(WsMessage::Ping(_)) | Ok(WsMessage::Pong(_)) => {
                    // Handled automatically by tokio-tungstenite
                }
                Ok(WsMessage::Close(_)) => {
                    log::info!("Server closed connection");
                    break;
                }
                Err(e) => {
                    log::error!("WebSocket error: {}", e);
                    break;
                }
                _ => {}
            }
        }
    }

    /// Receive next audio chunk
    pub async fn recv_audio_chunk(&mut self) -> Option<AudioChunk> {
        self.audio_rx.recv().await
    }

    /// Receive next protocol message
    pub async fn recv_message(&mut self) -> Option<Message> {
        self.message_rx.recv().await
    }

    /// Split into separate receivers for concurrent processing
    ///
    /// This allows using tokio::select! to process messages and audio chunks concurrently
    /// without borrow checker issues
    pub fn split(
        self,
    ) -> (
        UnboundedReceiver<Message>,
        UnboundedReceiver<AudioChunk>,
    ) {
        (self.message_rx, self.audio_rx)
    }
}
