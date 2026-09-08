// ABOUTME: Outbound writer task: serializes application messages onto the encrypted channel
// ABOUTME: Owns key-swap ordering for re-handshakes and the farewell-then-close teardown

//! The single writer task for a connection.
//!
//! All outbound traffic funnels through one task so wire order is exactly
//! queue order — which is what makes the re-handshake key swap and the
//! farewell-then-close teardown safe to express as queue commands.

use crate::error::Error;
use crate::protocol::client::DEFAULT_DISCONNECT_TIMEOUT;
use crate::protocol::messages::Message;
use crate::protocol::transport::{frame_type, EncryptedChannel};
use futures_util::{stream::SplitSink, SinkExt};
use parking_lot::Mutex;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_tungstenite::{tungstenite::Message as WsMessage, WebSocketStream};

/// An outbound application message.
pub(crate) enum OutboundPayload {
    /// A JSON message (binary message ID 0).
    Json(Box<Message>),
    /// A raw binary message (e.g. source audio chunks).
    Binary { msg_type: u8, payload: Vec<u8> },
}

/// `Farewell` is one variant (not `Send` + `Close`) so the writer processes it
/// atomically: once dequeued it flushes the farewell message + close frame and
/// exits, so nothing *enqueued after it* reaches the wire.
pub(crate) enum WriteCommand {
    Send {
        msg: OutboundPayload,
        ack: tokio::sync::oneshot::Sender<Result<(), Error>>,
    },
    /// Send Noise re-handshake message 2 under the *current* keys, then swap
    /// the shared channel to the new keys. Processed in queue order so
    /// nothing enqueued earlier is encrypted under the wrong key set.
    Rehandshake {
        msg2: Box<Message>,
        new_channel: Box<EncryptedChannel>,
    },
    /// Send a final message (`client/goodbye` or `pair/abort`), close the
    /// WebSocket, and exit. The complete farewell flush and close operation
    /// is bounded by `DEFAULT_DISCONNECT_TIMEOUT`.
    Farewell {
        msg: Box<Message>,
        ack: tokio::sync::oneshot::Sender<Result<(), Error>>,
    },
}

/// Encrypt one application message into Noise ciphertext frames.
fn encrypt_outbound(
    channel: &Mutex<EncryptedChannel>,
    payload: &OutboundPayload,
) -> Result<Vec<Vec<u8>>, Error> {
    match payload {
        OutboundPayload::Json(msg) => {
            let json = serde_json::to_vec(msg).map_err(|e| Error::Protocol(e.to_string()))?;
            channel.lock().encrypt_message(frame_type::JSON, &json)
        }
        OutboundPayload::Binary { msg_type, payload } => {
            channel.lock().encrypt_message(*msg_type, payload)
        }
    }
}

/// Send one application message over the encrypted channel.
pub(crate) async fn send_encrypted<S>(
    sink: &mut SplitSink<WebSocketStream<S>, WsMessage>,
    channel: &Mutex<EncryptedChannel>,
    payload: OutboundPayload,
) -> Result<(), Error>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    for frame in encrypt_outbound(channel, &payload)? {
        sink.send(WsMessage::Binary(frame.into()))
            .await
            .map_err(|e| Error::WebSocket(e.to_string()))?;
    }
    Ok(())
}

/// The writer loop. `_dead_on_drop` is dropped when this task exits (for any
/// reason, including a wire-write failure), which the message router observes
/// as a liveness signal: a connection whose writer has died is dead even if
/// its read half still looks idle-healthy.
pub(crate) async fn writer_task<S>(
    mut sink: SplitSink<WebSocketStream<S>, WsMessage>,
    channel: Arc<Mutex<EncryptedChannel>>,
    mut rx: UnboundedReceiver<WriteCommand>,
    _dead_on_drop: tokio::sync::oneshot::Sender<()>,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    'writer: while let Some(cmd) = rx.recv().await {
        match cmd {
            WriteCommand::Send { msg, ack } => {
                let result = send_encrypted(&mut sink, &channel, msg).await;
                let failed = result.is_err();
                // Ignore SendError: the caller may have dropped its receiver.
                let _ = ack.send(result);
                if failed {
                    break;
                }
            }
            WriteCommand::Rehandshake { msg2, new_channel } => {
                // Encrypt message 2 under the pre-re-handshake transport keys
                // before publishing the new channel. The protocol requires
                // quiescence during this exchange; publishing only after
                // encryption preserves the old-key ordering if the peer
                // receives/responds before the socket send future resolves.
                let frames = match encrypt_outbound(&channel, &OutboundPayload::Json(msg2)) {
                    Ok(frames) => frames,
                    Err(_) => break,
                };
                *channel.lock() = *new_channel;
                log::debug!("Re-handshake complete; transport keys swapped");
                for frame in frames {
                    if sink.send(WsMessage::Binary(frame.into())).await.is_err() {
                        break 'writer;
                    }
                }
            }
            WriteCommand::Farewell { msg, ack } => {
                let result = match tokio::time::timeout(
                    DEFAULT_DISCONNECT_TIMEOUT,
                    perform_farewell(&mut sink, &channel, *msg),
                )
                .await
                {
                    Ok(result) => result,
                    Err(_) => Err(Error::Connection(format!(
                        "farewell flush timed out after {DEFAULT_DISCONNECT_TIMEOUT:?}"
                    ))),
                };
                let _ = ack.send(result);
                break;
            }
        }
    }
    log::debug!("Writer task exiting");
    // On exit `rx` drops, dropping the ack sender of any still-queued command;
    // callers awaiting those acks see the cancellation and treat it as a closed
    // connection (see `WsSender::send_message`).
}

async fn perform_farewell<S>(
    sink: &mut SplitSink<WebSocketStream<S>, WsMessage>,
    channel: &Mutex<EncryptedChannel>,
    msg: Message,
) -> Result<(), Error>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    send_encrypted(sink, channel, OutboundPayload::Json(Box::new(msg))).await?;
    sink.close()
        .await
        .map_err(|e| Error::WebSocket(e.to_string()))
}
