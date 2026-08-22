// ABOUTME: Noise transport layer: handshake driver, encrypted channel, and fragmentation
// ABOUTME: Converts between application messages (type byte + payload) and Noise ciphertext frames

//! Encrypted transport for Sendspin connections.
//!
//! After the cleartext `client/init` / `server/init` / `noise/handshake`
//! exchange, every Sendspin application message travels as a WebSocket binary
//! frame whose payload is a Noise transport ciphertext. The first byte of the
//! AEAD plaintext is the binary message ID (`0` = JSON body); messages larger
//! than one Noise message are split across fragment frames (IDs 2 and 3).

use crate::error::Error;
use crate::protocol::crypto::{
    build_client_handshake, CipherSuite, Identity, Psk, AEAD_TAG_LEN, NOISE_MAX_MESSAGE,
};
use crate::Result;

/// Binary message IDs owned by the core protocol layer.
pub mod frame_type {
    /// JSON message body (UTF-8).
    pub const JSON: u8 = 0;
    /// Fragment-more frame (fragmented message, not the last fragment).
    pub const FRAGMENT_MORE: u8 = 2;
    /// Fragment-end frame (last fragment of a fragmented message).
    pub const FRAGMENT_END: u8 = 3;
}

/// Maximum AEAD plaintext per Noise transport message.
const MAX_PLAINTEXT: usize = NOISE_MAX_MESSAGE - AEAD_TAG_LEN; // 65519

/// Maximum application payload in a non-fragmented frame: type byte + payload.
const MAX_SINGLE_PAYLOAD: usize = MAX_PLAINTEXT - 1; // 65518

/// Maximum size of a reassembled fragmented message. The spec places no
/// limit on the logical message, so without a cap a peer could stream
/// fragment-more frames until the process runs out of memory. 16 MiB
/// comfortably covers the largest defined payloads (artwork images) while
/// bounding a malicious or broken peer.
const MAX_REASSEMBLED_LEN: usize = 16 * 1024 * 1024;

// =============================================================================
// Client-side handshake driver
// =============================================================================

/// Drives the client side (Noise responder) of the `KKpsk2` handshake.
///
/// The PSK is not known until Noise message 1 reveals its `psk_id`, so the
/// state is built with a placeholder PSK and the real PSK is installed via
/// `set_psk` before writing message 2 (the `psk2` position).
pub(crate) struct ClientHandshake {
    state: snow::HandshakeState,
    suite: CipherSuite,
    read_message_1: bool,
}

impl ClientHandshake {
    /// Create the responder handshake state.
    ///
    /// `prologue` is the exact wire bytes of `client/init` followed by
    /// `server/init` for the initial handshake, or the prior handshake's hash
    /// `h` for a re-handshake.
    pub fn new(
        suite: CipherSuite,
        identity: &Identity,
        server_public: &[u8; 32],
        prologue: &[u8],
    ) -> Result<Self> {
        // Placeholder PSK; replaced via set_psk before message 2 is written.
        let placeholder = Psk::new([0u8; 32]);
        let state = build_client_handshake(suite, identity, server_public, &placeholder, prologue)?;
        Ok(Self {
            state,
            suite,
            read_message_1: false,
        })
    }

    /// Process Noise message 1 (server → client) and return its decrypted
    /// payload bytes (a UTF-8 JSON object carrying `psk_id`).
    ///
    /// Message 1 is decryptable without the PSK mixed in (`psk2` pattern).
    pub fn read_message_1(&mut self, message: &[u8]) -> Result<Vec<u8>> {
        if self.read_message_1 {
            return Err(Error::Protocol("noise message 1 already processed".into()));
        }
        let mut payload = vec![0u8; NOISE_MAX_MESSAGE];
        let len = self
            .state
            .read_message(message, &mut payload)
            .map_err(|e| Error::Crypto(format!("noise message 1: {e}")))?;
        payload.truncate(len);
        self.read_message_1 = true;
        Ok(payload)
    }

    /// Install the PSK selected by `psk_id` and produce Noise message 2
    /// (client → server), whose encrypted payload is the literal two bytes
    /// `{}`.
    pub fn write_message_2(&mut self, psk: &Psk) -> Result<Vec<u8>> {
        if !self.read_message_1 {
            return Err(Error::Protocol(
                "noise message 2 requested before message 1".into(),
            ));
        }
        self.state
            .set_psk(2, psk.bytes())
            .map_err(|e| Error::Crypto(format!("noise set_psk: {e}")))?;
        let mut buf = vec![0u8; NOISE_MAX_MESSAGE];
        let len = self
            .state
            .write_message(b"{}", &mut buf)
            .map_err(|e| Error::Crypto(format!("noise message 2: {e}")))?;
        buf.truncate(len);
        Ok(buf)
    }

    /// Finish the handshake, switching to transport mode.
    pub fn into_channel(self) -> Result<EncryptedChannel> {
        if !self.state.is_handshake_finished() {
            return Err(Error::Protocol("noise handshake not finished".into()));
        }
        let mut handshake_hash = [0u8; 32];
        let hash = self.state.get_handshake_hash();
        if hash.len() != 32 {
            return Err(Error::Crypto("unexpected handshake hash length".into()));
        }
        handshake_hash.copy_from_slice(hash);
        let transport = self
            .state
            .into_transport_mode()
            .map_err(|e| Error::Crypto(format!("noise transport mode: {e}")))?;
        Ok(EncryptedChannel {
            transport,
            suite: self.suite,
            handshake_hash,
            reassembly: None,
        })
    }
}

// =============================================================================
// Encrypted channel
// =============================================================================

/// A Noise transport-mode channel carrying Sendspin application messages.
///
/// Handles AEAD encryption/decryption, the leading message-ID byte, and
/// fragmentation/reassembly (binary message IDs 2/3).
pub(crate) struct EncryptedChannel {
    transport: snow::TransportState,
    suite: CipherSuite,
    handshake_hash: [u8; 32],
    /// In-flight inbound fragmented message: (orig_type, accumulated data).
    reassembly: Option<(u8, Vec<u8>)>,
}

impl std::fmt::Debug for EncryptedChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EncryptedChannel")
            .field("suite", &self.suite)
            .field("reassembly_in_flight", &self.reassembly.is_some())
            .finish_non_exhaustive()
    }
}

impl EncryptedChannel {
    /// The handshake hash `h`, used as the prologue for a re-handshake.
    pub fn handshake_hash(&self) -> &[u8; 32] {
        &self.handshake_hash
    }

    /// Encrypt one application message into one or more WebSocket binary
    /// frame payloads (Noise ciphertexts), fragmenting when required.
    ///
    /// `msg_type` must not be a fragment type (2 or 3).
    pub fn encrypt_message(&mut self, msg_type: u8, payload: &[u8]) -> Result<Vec<Vec<u8>>> {
        if msg_type == frame_type::FRAGMENT_MORE || msg_type == frame_type::FRAGMENT_END {
            return Err(Error::Protocol(format!(
                "message type {msg_type} is reserved for fragmentation"
            )));
        }
        if payload.len() <= MAX_SINGLE_PAYLOAD {
            let mut plaintext = Vec::with_capacity(1 + payload.len());
            plaintext.push(msg_type);
            plaintext.extend_from_slice(payload);
            return Ok(vec![self.encrypt_plaintext(&plaintext)?]);
        }

        // Fragmented: opening fragment-more frame carries orig_type.
        let mut frames = Vec::new();
        let first_chunk_len = MAX_PLAINTEXT - 2; // [2][orig_type][data]
        let (first, mut rest) = payload.split_at(first_chunk_len);
        let mut plaintext = Vec::with_capacity(MAX_PLAINTEXT);
        plaintext.push(frame_type::FRAGMENT_MORE);
        plaintext.push(msg_type);
        plaintext.extend_from_slice(first);
        frames.push(self.encrypt_plaintext(&plaintext)?);

        // Continuation frames: [2][data]; final frame: [3][data].
        while rest.len() > MAX_SINGLE_PAYLOAD {
            let (chunk, tail) = rest.split_at(MAX_SINGLE_PAYLOAD);
            let mut plaintext = Vec::with_capacity(1 + chunk.len());
            plaintext.push(frame_type::FRAGMENT_MORE);
            plaintext.extend_from_slice(chunk);
            frames.push(self.encrypt_plaintext(&plaintext)?);
            rest = tail;
        }
        let mut plaintext = Vec::with_capacity(1 + rest.len());
        plaintext.push(frame_type::FRAGMENT_END);
        plaintext.extend_from_slice(rest);
        frames.push(self.encrypt_plaintext(&plaintext)?);
        Ok(frames)
    }

    /// Decrypt one inbound WebSocket binary frame payload.
    ///
    /// Returns `Ok(Some((msg_type, payload)))` when a complete message is
    /// available, `Ok(None)` while a fragmented message is still being
    /// reassembled, and `Err` on AEAD failure or a malformed fragment
    /// sequence — both of which MUST close the connection.
    pub fn decrypt_frame(&mut self, ciphertext: &[u8]) -> Result<Option<(u8, Vec<u8>)>> {
        let mut plaintext = vec![0u8; NOISE_MAX_MESSAGE];
        let len = self
            .transport
            .read_message(ciphertext, &mut plaintext)
            .map_err(|e| Error::Crypto(format!("noise transport decrypt: {e}")))?;
        plaintext.truncate(len);
        let (&msg_type, data) = plaintext
            .split_first()
            .ok_or_else(|| Error::Protocol("empty noise plaintext".into()))?;

        match (&mut self.reassembly, msg_type) {
            // No message in flight: a fragment-more frame begins one.
            (None, frame_type::FRAGMENT_MORE) => {
                let (&orig_type, first) = data.split_first().ok_or_else(|| {
                    Error::Protocol("opening fragment frame missing orig_type".into())
                })?;
                if orig_type == frame_type::FRAGMENT_MORE || orig_type == frame_type::FRAGMENT_END {
                    return Err(Error::Protocol(format!(
                        "invalid fragmented orig_type {orig_type}"
                    )));
                }
                self.reassembly = Some((orig_type, first.to_vec()));
                Ok(None)
            }
            // No message in flight: a fragment-end frame is a protocol error.
            (None, frame_type::FRAGMENT_END) => Err(Error::Protocol(
                "fragment-end frame with no fragmented message in flight".into(),
            )),
            // No message in flight: ordinary message.
            (None, _) => Ok(Some((msg_type, data.to_vec()))),
            // Message in flight: continuation.
            (Some((_, buffer)), frame_type::FRAGMENT_MORE) => {
                if buffer.len() + data.len() > MAX_REASSEMBLED_LEN {
                    self.reassembly = None;
                    return Err(Error::Protocol(format!(
                        "fragmented message exceeds {MAX_REASSEMBLED_LEN} bytes"
                    )));
                }
                buffer.extend_from_slice(data);
                Ok(None)
            }
            // Message in flight: final fragment dispatches the message.
            (Some(_), frame_type::FRAGMENT_END) => {
                let (orig_type, mut buffer) = self.reassembly.take().expect("checked in-flight");
                if buffer.len() + data.len() > MAX_REASSEMBLED_LEN {
                    return Err(Error::Protocol(format!(
                        "fragmented message exceeds {MAX_REASSEMBLED_LEN} bytes"
                    )));
                }
                buffer.extend_from_slice(data);
                Ok(Some((orig_type, buffer)))
            }
            // Message in flight: any non-fragment frame is a protocol error.
            (Some(_), other) => Err(Error::Protocol(format!(
                "non-fragment frame (type {other}) while a fragmented message is in flight"
            ))),
        }
    }

    fn encrypt_plaintext(&mut self, plaintext: &[u8]) -> Result<Vec<u8>> {
        debug_assert!(plaintext.len() <= MAX_PLAINTEXT);
        let mut buf = vec![0u8; plaintext.len() + AEAD_TAG_LEN];
        let len = self
            .transport
            .write_message(plaintext, &mut buf)
            .map_err(|e| Error::Crypto(format!("noise transport encrypt: {e}")))?;
        buf.truncate(len);
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::crypto::{build_server_handshake, Identity, Psk};

    /// Complete a handshake between a simulated server initiator and the
    /// ClientHandshake driver, returning both transport channels.
    fn establish(suite: CipherSuite) -> (snow::TransportState, EncryptedChannel) {
        let client_identity = Identity::generate().unwrap();
        let server_identity = Identity::generate().unwrap();
        let psk = Psk::sentinel();
        let prologue = b"init-bytes";

        let mut server = build_server_handshake(
            suite,
            &server_identity,
            client_identity.public_bytes(),
            &psk,
            prologue,
        )
        .unwrap();
        let mut client = ClientHandshake::new(
            suite,
            &client_identity,
            server_identity.public_bytes(),
            prologue,
        )
        .unwrap();

        let mut buf = [0u8; NOISE_MAX_MESSAGE];
        let msg1_payload = format!("{{\"psk_id\":\"{}\"}}", psk.psk_id());
        let len = server
            .write_message(msg1_payload.as_bytes(), &mut buf)
            .unwrap();
        let payload = client.read_message_1(&buf[..len]).unwrap();
        assert_eq!(payload, msg1_payload.as_bytes());

        let msg2 = client.write_message_2(&psk).unwrap();
        let mut payload = [0u8; NOISE_MAX_MESSAGE];
        let plen = server.read_message(&msg2, &mut payload).unwrap();
        assert_eq!(&payload[..plen], b"{}");

        let server_hash = server.get_handshake_hash().to_vec();
        let server_transport = server.into_transport_mode().unwrap();
        let channel = client.into_channel().unwrap();
        assert_eq!(channel.handshake_hash(), server_hash.as_slice());
        (server_transport, channel)
    }

    fn server_send(server: &mut snow::TransportState, msg_type: u8, payload: &[u8]) -> Vec<u8> {
        let mut plaintext = Vec::with_capacity(1 + payload.len());
        plaintext.push(msg_type);
        plaintext.extend_from_slice(payload);
        let mut buf = vec![0u8; plaintext.len() + AEAD_TAG_LEN];
        let len = server.write_message(&plaintext, &mut buf).unwrap();
        buf.truncate(len);
        buf
    }

    #[test]
    fn deferred_psk_handshake_completes_on_both_suites() {
        for suite in [CipherSuite::ChaChaPoly, CipherSuite::AesGcm] {
            let (_server, _channel) = establish(suite);
        }
    }

    #[test]
    fn small_message_round_trip() {
        let (mut server, mut channel) = establish(CipherSuite::ChaChaPoly);

        // Client -> server.
        let frames = channel
            .encrypt_message(frame_type::JSON, b"{\"x\":1}")
            .unwrap();
        assert_eq!(frames.len(), 1);
        let mut plaintext = [0u8; NOISE_MAX_MESSAGE];
        let len = server.read_message(&frames[0], &mut plaintext).unwrap();
        assert_eq!(&plaintext[..len], b"\x00{\"x\":1}");

        // Server -> client.
        let frame = server_send(&mut server, 4, b"audio-bytes");
        let (msg_type, payload) = channel.decrypt_frame(&frame).unwrap().unwrap();
        assert_eq!(msg_type, 4);
        assert_eq!(payload, b"audio-bytes");
    }

    #[test]
    fn outbound_fragmentation_splits_and_reassembles() {
        let (mut server, mut channel) = establish(CipherSuite::ChaChaPoly);

        // Payload larger than two full frames to force >2 fragments.
        let payload: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let frames = channel.encrypt_message(frame_type::JSON, &payload).unwrap();
        assert!(frames.len() >= 3, "expected at least 3 fragments");

        // Reassemble server-side per the spec's receiver algorithm.
        let mut reassembled: Vec<u8> = Vec::new();
        let mut orig_type: Option<u8> = None;
        let mut plaintext = [0u8; NOISE_MAX_MESSAGE];
        for (i, frame) in frames.iter().enumerate() {
            let len = server.read_message(frame, &mut plaintext).unwrap();
            assert!(len <= NOISE_MAX_MESSAGE - AEAD_TAG_LEN);
            let ftype = plaintext[0];
            let last = i == frames.len() - 1;
            if last {
                assert_eq!(ftype, frame_type::FRAGMENT_END);
                reassembled.extend_from_slice(&plaintext[1..len]);
            } else if i == 0 {
                assert_eq!(ftype, frame_type::FRAGMENT_MORE);
                orig_type = Some(plaintext[1]);
                reassembled.extend_from_slice(&plaintext[2..len]);
            } else {
                assert_eq!(ftype, frame_type::FRAGMENT_MORE);
                reassembled.extend_from_slice(&plaintext[1..len]);
            }
        }
        assert_eq!(orig_type, Some(frame_type::JSON));
        assert_eq!(reassembled, payload);
    }

    #[test]
    fn inbound_fragmentation_reassembles() {
        let (mut server, mut channel) = establish(CipherSuite::ChaChaPoly);

        // Server sends a fragmented type-8 (artwork) message in 3 frames.
        let f1 = server_send(&mut server, frame_type::FRAGMENT_MORE, &[8, 1, 2, 3]);
        let f2 = server_send(&mut server, frame_type::FRAGMENT_MORE, &[4, 5]);
        let f3 = server_send(&mut server, frame_type::FRAGMENT_END, &[6]);

        assert!(channel.decrypt_frame(&f1).unwrap().is_none());
        assert!(channel.decrypt_frame(&f2).unwrap().is_none());
        let (msg_type, payload) = channel.decrypt_frame(&f3).unwrap().unwrap();
        assert_eq!(msg_type, 8);
        assert_eq!(payload, vec![1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn fragment_end_without_in_flight_is_protocol_error() {
        let (mut server, mut channel) = establish(CipherSuite::ChaChaPoly);
        let frame = server_send(&mut server, frame_type::FRAGMENT_END, b"x");
        assert!(channel.decrypt_frame(&frame).is_err());
    }

    #[test]
    fn non_fragment_during_in_flight_is_protocol_error() {
        let (mut server, mut channel) = establish(CipherSuite::ChaChaPoly);
        let f1 = server_send(&mut server, frame_type::FRAGMENT_MORE, &[8, 1]);
        assert!(channel.decrypt_frame(&f1).unwrap().is_none());
        let bad = server_send(&mut server, frame_type::JSON, b"{}");
        assert!(channel.decrypt_frame(&bad).is_err());
    }

    #[test]
    fn fragmented_orig_type_2_or_3_is_protocol_error() {
        let (mut server, mut channel) = establish(CipherSuite::ChaChaPoly);
        let frame = server_send(&mut server, frame_type::FRAGMENT_MORE, &[3, 1, 2]);
        assert!(channel.decrypt_frame(&frame).is_err());
    }

    #[test]
    fn oversized_reassembly_is_rejected() {
        let (mut server, mut channel) = establish(CipherSuite::ChaChaPoly);
        let chunk = vec![0u8; MAX_SINGLE_PAYLOAD - 1];
        let f1 = server_send(
            &mut server,
            frame_type::FRAGMENT_MORE,
            &[&[8u8][..], &chunk].concat(),
        );
        assert!(channel.decrypt_frame(&f1).unwrap().is_none());
        let mut rejected = false;
        for _ in 0..(MAX_REASSEMBLED_LEN / chunk.len() + 2) {
            let frame = server_send(&mut server, frame_type::FRAGMENT_MORE, &chunk);
            match channel.decrypt_frame(&frame) {
                Ok(None) => continue,
                Ok(Some(_)) => panic!("no fragment-end was sent"),
                Err(_) => {
                    rejected = true;
                    break;
                }
            }
        }
        assert!(rejected, "reassembly cap never triggered");
    }

    #[test]
    fn sender_rejects_fragment_types_as_orig_type() {
        let (_server, mut channel) = establish(CipherSuite::ChaChaPoly);
        assert!(channel.encrypt_message(2, b"x").is_err());
        assert!(channel.encrypt_message(3, b"x").is_err());
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let (mut server, mut channel) = establish(CipherSuite::ChaChaPoly);
        let mut frame = server_send(&mut server, frame_type::JSON, b"{}");
        let last = frame.len() - 1;
        frame[last] ^= 0xff;
        assert!(channel.decrypt_frame(&frame).is_err());
    }

    #[test]
    fn message_2_before_message_1_is_rejected() {
        let client_identity = Identity::generate().unwrap();
        let server_identity = Identity::generate().unwrap();
        let mut hs = ClientHandshake::new(
            CipherSuite::ChaChaPoly,
            &client_identity,
            server_identity.public_bytes(),
            b"p",
        )
        .unwrap();
        assert!(hs.write_message_2(&Psk::sentinel()).is_err());
    }
}
