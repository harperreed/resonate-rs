// ABOUTME: Pairing support: pairing tokens (SP: base32 encoding) and the pairing record store
// ABOUTME: Implements the Pairing PSK flow's client-side persistence and token emission

//! Pairing tokens and pairing-record storage.
//!
//! A **pairing token** is a single case-insensitive ASCII string carrying a
//! pairing secret, transferred out of band (copy/paste, QR scan) from the
//! client into the server: `"SP:" || version || body`, where `body` is RFC
//! 4648 base32 with padding stripped and every `2` transliterated to `9`.
//!
//! Version `0` carries `client_key (32 bytes) || pairing_psk (32 bytes)` —
//! the Pairing PSK distribution format. Version `1` carries a per-session
//! 24-byte dynamic pairing code (QR emission; not yet implemented here).
//!
//! The **pairing record store** persists long-term PSKs established by
//! pairing, each bound to the `server_id` it was established with. A client
//! must be able to store at least 5 records; when a pairing completes at
//! capacity the client evicts an existing record (implementation-defined,
//! but never one backing a currently open connection) so the new record
//! persists — a pairing never fails for lack of record storage.

use crate::error::Error;
use crate::protocol::crypto::{b64url_encode, Identity, Psk, PskCandidate, PskCategory};
use crate::Result;
use parking_lot::Mutex;
use std::sync::Arc;

const BASE32_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// Encode a payload as a pairing-token body: RFC 4648 base32, `=` padding
/// stripped, every `2` transliterated to `9`.
fn encode_body(payload: &[u8]) -> String {
    let mut out = String::with_capacity(payload.len().div_ceil(5) * 8);
    for chunk in payload.chunks(5) {
        let mut block = [0u8; 5];
        block[..chunk.len()].copy_from_slice(chunk);
        let bits = u64::from(block[0]) << 32
            | u64::from(block[1]) << 24
            | u64::from(block[2]) << 16
            | u64::from(block[3]) << 8
            | u64::from(block[4]);
        // 8 output chars per 5-byte group; truncate to the used chars.
        let chars = match chunk.len() {
            1 => 2,
            2 => 4,
            3 => 5,
            4 => 7,
            _ => 8,
        };
        for i in 0..chars {
            let shift = 35 - 5 * i;
            let idx = ((bits >> shift) & 0x1f) as usize;
            out.push(BASE32_ALPHABET[idx] as char);
        }
    }
    // QR-alphanumeric-friendly transliteration.
    out.replace('2', "9")
}

/// Decode a pairing-token body back into payload bytes (lenient input rules
/// are applied by [`decode_token`]; this expects the bare upper-cased body).
fn decode_body(body: &str) -> Result<Vec<u8>> {
    let restored = body.replace('9', "2");
    let mut bits: u64 = 0;
    let mut nbits = 0u32;
    let mut out = Vec::with_capacity(restored.len() * 5 / 8);
    for c in restored.bytes() {
        let val = match c {
            b'A'..=b'Z' => c - b'A',
            b'2'..=b'7' => c - b'2' + 26,
            _ => {
                return Err(Error::Protocol(format!(
                    "invalid base32 char '{}'",
                    c as char
                )))
            }
        };
        bits = (bits << 5) | u64::from(val);
        nbits += 5;
        if nbits >= 8 {
            nbits -= 8;
            out.push(((bits >> nbits) & 0xff) as u8);
        }
    }
    // Leftover bits are padding and must represent zero per RFC 4648.
    if nbits > 0 && (bits & ((1 << nbits) - 1)) != 0 {
        return Err(Error::Protocol("non-zero base32 padding bits".to_string()));
    }
    Ok(out)
}

/// Build a version-0 pairing token: `SP:0` + base32 body of
/// `client_key (32) || pairing_psk (32)`.
///
/// A client displays or transfers this sensitive token so the operator can
/// enter it into a server, which then verifies the embedded `client_key`
/// against the connection's `client_id` and runs the Pairing PSK flow.
pub fn pairing_psk_token(identity: &Identity, pairing_psk: &Psk) -> String {
    let mut payload = Vec::with_capacity(64);
    payload.extend_from_slice(identity.public_bytes());
    payload.extend_from_slice(pairing_psk.bytes());
    format!("SP:0{}", encode_body(&payload))
}

/// A decoded pairing token.
#[derive(Clone, PartialEq, Eq)]
pub enum PairingToken {
    /// Version 0: a Pairing PSK with the client identity.
    PairingPsk {
        /// The client's raw 32-byte Curve25519 public key.
        client_key: [u8; 32],
        /// The raw 32-byte Pairing PSK.
        pairing_psk: [u8; 32],
    },
    /// Version 1: a 24-byte dynamic pairing code (QR emission format).
    DynamicCode {
        /// The raw 24-byte pairing code.
        code: [u8; 24],
    },
}

impl std::fmt::Debug for PairingToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PairingPsk { client_key, .. } => f
                .debug_struct("PairingPsk")
                .field("client_key", &b64url_encode(client_key))
                .field("pairing_psk", &"[redacted]")
                .finish(),
            Self::DynamicCode { .. } => f
                .debug_struct("DynamicCode")
                .field("code", &"[redacted]")
                .finish(),
        }
    }
}

/// Decode operator-supplied pairing-token input, applying the spec's lenient
/// rules: trim whitespace, upper-case, optional `SP:` prefix, version
/// dispatch, and payload-length checks (extra payload bytes are ignored).
pub fn decode_token(input: &str) -> Result<PairingToken> {
    let trimmed = input.trim().to_ascii_uppercase();
    let rest = trimmed.strip_prefix("SP:").unwrap_or(&trimmed);
    let mut chars = rest.chars();
    let version = chars
        .next()
        .ok_or_else(|| Error::Protocol("empty pairing token".to_string()))?;
    let body = chars.as_str();
    let payload = decode_body(body)?;
    match version {
        '0' => {
            if payload.len() < 64 {
                return Err(Error::Protocol(
                    "version-0 token payload too short".to_string(),
                ));
            }
            let mut client_key = [0u8; 32];
            client_key.copy_from_slice(&payload[..32]);
            let mut pairing_psk = [0u8; 32];
            pairing_psk.copy_from_slice(&payload[32..64]);
            Ok(PairingToken::PairingPsk {
                client_key,
                pairing_psk,
            })
        }
        '1' => {
            if payload.len() < 24 {
                return Err(Error::Protocol(
                    "version-1 token payload too short".to_string(),
                ));
            }
            let mut code = [0u8; 24];
            code.copy_from_slice(&payload[..24]);
            Ok(PairingToken::DynamicCode { code })
        }
        other => Err(Error::Protocol(format!(
            "unrecognized pairing token version '{other}'"
        ))),
    }
}

// =============================================================================
// Pairing record store
// =============================================================================

/// A persisted pairing record holding a long-term PSK.
#[derive(Debug, Clone)]
pub struct PairingRecord {
    /// The long-term PSK.
    pub psk: Psk,
    /// The server identity the record was established with. After a `psk_id`
    /// match, the client verifies this equals the connection's `server_id`.
    pub server_id: String,
    /// True once a server has authenticated a session with this record's PSK.
    pub used: bool,
}

impl PairingRecord {
    /// The record's identifier.
    pub fn psk_id(&self) -> String {
        self.psk.psk_id()
    }

    /// The PSK candidate this record contributes to handshakes.
    pub fn candidate(&self) -> PskCandidate {
        PskCandidate {
            psk: self.psk.clone(),
            category: PskCategory::LongTerm {
                server_id: self.server_id.clone(),
            },
        }
    }
}

/// Outcome of a store mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreError {
    /// A PSK with the same psk_id already exists (any category).
    AlreadyExists,
    /// The referenced record does not exist.
    NotFound,
    /// The store cannot persist the change.
    StorageExhausted,
    /// A referential constraint forbids the operation.
    Invalid,
}

/// Persistence for pairing records.
///
/// Implementations must be cheap to call from the connection router; blocking
/// I/O should be deferred or buffered by the implementation. The library
/// ships [`MemoryPairingStore`]; applications persist records by providing
/// their own implementation.
///
/// Bounded application-provided stores must hold at least 5 records, and when
/// `add_record` is called at capacity, evict an existing record (which one is
/// implementation-defined — e.g. least recently `mark_used` — but never one
/// backing a currently open connection) rather than failing. An evicted
/// server's next handshake lands in the Sentinel Fallback and it can offer its
/// operator re-pairing. The in-memory store shipped here is intentionally
/// unbounded; applications needing bounded persistence should provide a store
/// with the required eviction policy.
///
/// A record for a server replaces that server's previous record. A PSK ID
/// collision with a record belonging to another server remains an error.
pub trait PairingStore: Send + Sync {
    /// All records, in stable order.
    fn records(&self) -> Vec<PairingRecord>;
    /// Add or replace a record. Fails with `AlreadyExists` when the psk_id
    /// collides with an existing record belonging to another server, and with
    /// `StorageExhausted` when full and no record can be evicted.
    fn add_record(&self, record: PairingRecord) -> std::result::Result<(), StoreError>;
    /// Remove a record by psk_id.
    fn remove_record(&self, psk_id: &str) -> std::result::Result<(), StoreError>;
    /// Mark a record used after a successful authenticated session.
    fn mark_used(&self, psk_id: &str);
}

/// In-memory [`PairingStore`] (records do not survive process restart).
#[derive(Default)]
pub struct MemoryPairingStore {
    records: Mutex<Vec<PairingRecord>>,
}

impl MemoryPairingStore {
    /// Create an empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a store pre-seeded with records.
    pub fn with_records(records: Vec<PairingRecord>) -> Self {
        Self {
            records: Mutex::new(records),
        }
    }
}

impl PairingStore for MemoryPairingStore {
    fn records(&self) -> Vec<PairingRecord> {
        self.records.lock().clone()
    }

    fn add_record(&self, record: PairingRecord) -> std::result::Result<(), StoreError> {
        let mut records = self.records.lock();
        let psk_id = record.psk_id();
        if records
            .iter()
            .any(|r| r.psk_id() == psk_id && r.server_id != record.server_id)
        {
            return Err(StoreError::AlreadyExists);
        }
        if let Some(index) = records.iter().position(|r| r.server_id == record.server_id) {
            records[index] = record;
        } else {
            records.push(record);
        }
        Ok(())
    }

    fn remove_record(&self, psk_id: &str) -> std::result::Result<(), StoreError> {
        let mut records = self.records.lock();
        let before = records.len();
        records.retain(|r| r.psk_id() != psk_id);
        if records.len() == before {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    fn mark_used(&self, psk_id: &str) {
        let mut records = self.records.lock();
        if let Some(record) = records.iter_mut().find(|r| r.psk_id() == psk_id) {
            record.used = true;
        }
    }
}

/// Assemble the handshake PSK candidate set from a store plus the fixed
/// candidates (Sentinel and, for every conforming client, its Pairing PSK).
pub fn candidates_from(
    store: &Arc<dyn PairingStore>,
    pairing_psk: Option<&Psk>,
) -> Vec<PskCandidate> {
    let mut candidates = vec![PskCandidate {
        psk: Psk::sentinel(),
        category: PskCategory::Sentinel,
    }];
    if let Some(psk) = pairing_psk {
        candidates.push(PskCandidate {
            psk: psk.clone(),
            category: PskCategory::Pairing,
        });
    }
    candidates.extend(store.records().iter().map(PairingRecord::candidate));
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spec reference vector: client_key = 0x00..0x1f, pairing_psk = 0xe0..0xff.
    #[test]
    fn version_0_token_matches_spec_vector() {
        // We need an Identity whose *public* key is 0x00..0x1f; Identity
        // derives public from secret, so build the token payload manually
        // through the encoder instead.
        let client_key: Vec<u8> = (0x00..=0x1f).collect();
        let pairing_psk: Vec<u8> = (0xe0..=0xff).collect();
        let mut payload = client_key.clone();
        payload.extend_from_slice(&pairing_psk);
        let token = format!("SP:0{}", encode_body(&payload));
        assert_eq!(
            token,
            "SP:0AAAQEAYEAUDAOCAJBIFQYDIOB4IBCEQTCQKRMFYYDENBWHA5DYP6BYPC4PSOLZXH5DU6V97M5XXO74HR6LZ7J5PW674PT6X37T6757Y"
        );
        // And decoding round-trips.
        let decoded = decode_token(&token).unwrap();
        assert_eq!(
            decoded,
            PairingToken::PairingPsk {
                client_key: <[u8; 32]>::try_from(client_key.as_slice()).unwrap(),
                pairing_psk: <[u8; 32]>::try_from(pairing_psk.as_slice()).unwrap(),
            }
        );
    }

    /// Spec reference vector for the version-1 token: code = 0xe0..0xf7.
    #[test]
    fn version_1_token_matches_spec_vector() {
        let code: Vec<u8> = (0xe0..=0xf7).collect();
        let token = format!("SP:1{}", encode_body(&code));
        assert_eq!(token, "SP:14DQ6FY7E4XTOP9HJ5LV6Z3PO57YPD4XT6T97N5Y");
        let decoded = decode_token(&token).unwrap();
        assert_eq!(
            decoded,
            PairingToken::DynamicCode {
                code: <[u8; 24]>::try_from(code.as_slice()).unwrap()
            }
        );
    }

    #[test]
    fn decode_is_lenient() {
        let code: Vec<u8> = (0xe0..=0xf7).collect();
        let token = format!("  sp:1{}  ", encode_body(&code).to_lowercase());
        assert!(decode_token(&token).is_ok());
        // Missing prefix is accepted too.
        let bare = format!("1{}", encode_body(&code));
        assert!(decode_token(&bare).is_ok());
        // Unknown version rejected.
        assert!(decode_token("SP:ZAAAA").is_err());
        // Short payload rejected.
        assert!(decode_token("SP:0AAAA").is_err());
    }

    #[test]
    fn decoded_pairing_psk_debug_redacts_secret() {
        let token = PairingToken::PairingPsk {
            client_key: [1u8; 32],
            pairing_psk: [2u8; 32],
        };
        let debug = format!("{token:?}");
        assert!(!debug.contains("2, 2, 2"));
        assert!(debug.contains("redacted"));
    }

    #[test]
    fn store_replaces_same_server_and_rejects_cross_server_psk_collision() {
        let store = MemoryPairingStore::new();
        let record = PairingRecord {
            psk: Psk::new([1u8; 32]),
            server_id: "server-a".to_string(),
            used: false,
        };
        store.add_record(record.clone()).unwrap();
        let replacement = PairingRecord {
            psk: Psk::new([2u8; 32]),
            server_id: "server-a".to_string(),
            used: true,
        };
        store.add_record(replacement.clone()).unwrap();
        assert_eq!(store.records().len(), 1);
        assert_eq!(store.records()[0].psk_id(), replacement.psk_id());
        assert_eq!(
            store.add_record(PairingRecord {
                psk: Psk::new([2u8; 32]),
                server_id: "server-b".to_string(),
                used: false,
            }),
            Err(StoreError::AlreadyExists)
        );
        let replacement_id = replacement.psk_id();
        store.mark_used(&replacement_id);
        assert!(store.records()[0].used);
        store.remove_record(&replacement_id).unwrap();
        assert_eq!(
            store.remove_record(&replacement_id),
            Err(StoreError::NotFound)
        );
    }

    #[test]
    fn candidates_include_sentinel_pairing_and_records() {
        let store: Arc<dyn PairingStore> =
            Arc::new(MemoryPairingStore::with_records(vec![PairingRecord {
                psk: Psk::new([1u8; 32]),
                server_id: "server-a".to_string(),
                used: false,
            }]));
        let pairing = Psk::new([2u8; 32]);
        let candidates = candidates_from(&store, Some(&pairing));
        assert_eq!(candidates.len(), 3);
        assert_eq!(candidates[0].category, PskCategory::Sentinel);
        assert_eq!(candidates[1].category, PskCategory::Pairing);
        assert_eq!(
            candidates[2].category,
            PskCategory::LongTerm {
                server_id: "server-a".to_string()
            }
        );
    }
}
