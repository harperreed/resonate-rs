// ABOUTME: WebSocket client implementation for Resonate protocol
// ABOUTME: Handles connection, message routing, and protocol state machine

use crate::error::Error;
use crate::protocol::messages::{ClientHello, Message};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};

/// WebSocket client for Resonate protocol
pub struct ProtocolClient {
    // Placeholder for now - will be fleshed out in Phase 2
}

impl ProtocolClient {
    /// Connect to Resonate server
    pub async fn connect(url: &str, hello: ClientHello) -> Result<Self, Error> {
        // Connect WebSocket
        let (ws_stream, _) = connect_async(url)
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        let (mut write, mut read) = ws_stream.split();

        // Send client hello
        let hello_msg = Message::ClientHello(hello);
        let hello_json =
            serde_json::to_string(&hello_msg).map_err(|e| Error::Protocol(e.to_string()))?;

        write
            .send(WsMessage::Text(hello_json))
            .await
            .map_err(|e| Error::WebSocket(e.to_string()))?;

        // Wait for server hello
        if let Some(Ok(WsMessage::Text(text))) = read.next().await {
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

        Ok(Self {})
    }
}
