// ABOUTME: Cryptographic foundation for the mandatory Sendspin Noise layer
// ABOUTME: Identities, PSK categories/derivation, cipher suites, and KKpsk2 handshake construction

//! Cryptographic building blocks for Sendspin's mandatory end-to-end encryption.
//!
//! All Sendspin connections run the `KKpsk2` Noise pattern. The **server is the
//! Noise initiator** and the **client is the Noise responder**, regardless of
//! which side opened the WebSocket. Static Curve25519 public keys double as the
//! wire identities (`client_id` / `server_id`, base64url without padding), and a
//! pre-shared key selected by `psk_id` is mixed in at the end of the second
//! handshake message.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use sha2::{Digest, Sha256};

use crate::error::Error;
use crate::Result;

/// Label prefixed to a PSK when deriving its `psk_id` (see spec: Pre-Shared Key).
const PSK_ID_LABEL: &[u8] = b"sendspin-psk-id-v1";

/// The published Sentinel PSK: `SHA-256("sendspin-sentinel-psk-v1")`.
///
/// Used as the PSK input whenever no other PSK applies (i.e., before any
/// pairing record exists). Provides no authentication on its own.
pub const SENTINEL_PSK: [u8; 32] = [
    0x1b, 0x5e, 0x24, 0xdb, 0xc1, 0xae, 0xd9, 0x5f, 0xc2, 0xa5, 0xa3, 0x38, 0xa9, 0x0c, 0x05, 0xdf,
    0x44, 0xbd, 0x10, 0xf5, 0xec, 0x1f, 0x4c, 0xd6, 0x6c, 0xbf, 0x86, 0x27, 0x27, 0x67, 0xb9, 0xd3,
];

/// base64url `psk_id` of the [`SENTINEL_PSK`] (published constant).
pub const SENTINEL_PSK_ID: &str = "GFsV9tLaSQm9HcFWpKsgYQOr7wFTvNUtkmFwuVz3zoo";

/// Encode bytes as base64url without padding (the encoding used for all
/// identity, PSK, and handshake fields on the wire).
pub fn b64url_encode(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Decode a base64url (no padding) string.
pub fn b64url_decode(s: &str) -> Result<Vec<u8>> {
    URL_SAFE_NO_PAD
        .decode(s)
        .map_err(|e| Error::Crypto(format!("invalid base64url: {e}")))
}

/// Decode a base64url string that must contain exactly 32 bytes
/// (identities, PSKs, and psk_id hashes are all 32-byte values, 43 chars).
pub fn b64url_decode_32(s: &str) -> Result<[u8; 32]> {
    let bytes = b64url_decode(s)?;
    <[u8; 32]>::try_from(bytes.as_slice())
        .map_err(|_| Error::Crypto(format!("expected 32 bytes, got {}", bytes.len())))
}

// =============================================================================
// Identity
// =============================================================================

/// A long-lived Curve25519 static keypair identifying a client or server.
///
/// The base64url-encoded public key (43 characters, no padding) serves as the
/// `client_id` or `server_id`. Applications should persist the secret key so
/// the identity survives reboots; rotating the keypair changes the identity.
#[derive(Clone)]
pub struct Identity {
    secret: [u8; 32],
    public: [u8; 32],
}

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Identity")
            .field("id", &self.id())
            .finish_non_exhaustive()
    }
}

impl Identity {
    /// Generate a fresh identity from the OS CSPRNG.
    pub fn generate() -> Result<Self> {
        let mut secret = [0u8; 32];
        getrandom::fill(&mut secret).map_err(|e| Error::Crypto(format!("CSPRNG failure: {e}")))?;
        Ok(Self::from_secret_bytes(secret))
    }

    /// Reconstruct an identity from a persisted 32-byte secret key.
    pub fn from_secret_bytes(secret: [u8; 32]) -> Self {
        let static_secret = x25519_dalek::StaticSecret::from(secret);
        let public = x25519_dalek::PublicKey::from(&static_secret);
        Self {
            secret,
            public: public.to_bytes(),
        }
    }

    /// The identity string: base64url-encoded public key (43 chars, no padding).
    pub fn id(&self) -> String {
        b64url_encode(&self.public)
    }

    /// Raw 32-byte public key.
    pub fn public_bytes(&self) -> &[u8; 32] {
        &self.public
    }

    /// Raw 32-byte secret key (for persistence).
    pub fn secret_bytes(&self) -> &[u8; 32] {
        &self.secret
    }
}

// =============================================================================
// Pre-shared keys
// =============================================================================

/// Stable device credentials required by every spec-conforming client.
///
/// This bundles the client's long-lived Noise identity and Pairing PSK because
/// the pairing token binds them together. The library never persists this
/// value; applications should persist [`Self::to_bytes`] and restore it with
/// [`Self::from_bytes`] before constructing a client.
#[derive(Clone)]
pub struct ClientCredentials {
    identity: Identity,
    pairing_psk: Psk,
}

impl std::fmt::Debug for ClientCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientCredentials")
            .field("client_id", &self.client_id())
            .field("pairing_psk_id", &self.pairing_psk.psk_id())
            .finish_non_exhaustive()
    }
}

impl ClientCredentials {
    const FORMAT_VERSION: u8 = 1;
    /// Serialized length: version byte plus two 32-byte secrets.
    pub const SERIALIZED_LEN: usize = 65;

    /// Generate fresh per-device credentials from the OS CSPRNG.
    pub fn generate() -> Result<Self> {
        Ok(Self {
            identity: Identity::generate()?,
            pairing_psk: Psk::generate()?,
        })
    }

    /// Construct credentials from already-persisted cryptographic parts.
    pub fn from_parts(identity: Identity, pairing_psk: Psk) -> Self {
        Self {
            identity,
            pairing_psk,
        }
    }

    /// Restore credentials from the versioned binary persistence format.
    ///
    /// The current format is exactly 65 bytes:
    /// `version (1 byte) || identity secret (32 bytes) || Pairing PSK (32 bytes)`.
    /// The input must have the exact length and supported format version.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != Self::SERIALIZED_LEN {
            return Err(Error::Crypto(format!(
                "client credentials must be {} bytes, got {}",
                Self::SERIALIZED_LEN,
                bytes.len()
            )));
        }
        if bytes[0] != Self::FORMAT_VERSION {
            return Err(Error::Crypto(format!(
                "unsupported client credentials version {}",
                bytes[0]
            )));
        }
        if bytes[1..33].iter().all(|&byte| byte == 0) || bytes[33..65].iter().all(|&byte| byte == 0)
        {
            return Err(Error::Crypto(
                "client credentials contain an all-zero key or PSK".to_string(),
            ));
        }
        let identity =
            Identity::from_secret_bytes(bytes[1..33].try_into().expect("length checked"));
        let pairing_psk = Psk::new(bytes[33..65].try_into().expect("length checked"));
        Ok(Self::from_parts(identity, pairing_psk))
    }

    /// Serialize credentials for application-owned secure storage.
    ///
    /// The returned bytes contain private key material and should be protected
    /// by the application's chosen credential store.
    pub fn to_bytes(&self) -> [u8; Self::SERIALIZED_LEN] {
        let mut bytes = [0u8; Self::SERIALIZED_LEN];
        bytes[0] = Self::FORMAT_VERSION;
        bytes[1..33].copy_from_slice(self.identity.secret_bytes());
        bytes[33..65].copy_from_slice(self.pairing_psk.bytes());
        bytes
    }

    /// The client's stable public identity string.
    pub fn client_id(&self) -> String {
        self.identity.id()
    }

    /// Access the underlying Noise identity.
    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    /// Access the reusable Pairing PSK.
    pub fn pairing_psk(&self) -> &Psk {
        &self.pairing_psk
    }

    /// Produce the required version-0 pairing token for operator export.
    ///
    /// The token contains the Pairing PSK, so treat it as sensitive while it is
    /// displayed or transferred to the operator/server.
    pub fn pairing_token(&self) -> String {
        crate::protocol::pairing::pairing_psk_token(&self.identity, &self.pairing_psk)
    }
}

/// The category a PSK belongs to. The client stores each PSK tagged with its
/// category; Noise message 1 declares the category the server is using the
/// referenced PSK as, and a match binds both sides to the same category.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PskCategory {
    /// The published Sentinel PSK (no authentication; unpaired connections).
    Sentinel,
    /// The client's pairing PSK (admits only the `['pairing']` activity set).
    Pairing,
    /// A long-term PSK from a pairing record, bound to the server identity
    /// it was established with.
    LongTerm {
        /// The bound server identity.
        server_id: String,
    },
}

impl PskCategory {
    /// The wire code (`'lt'` / `'pr'` / `'sn'`) this category matches.
    pub fn wire(&self) -> crate::protocol::messages::PskCategory {
        use crate::protocol::messages::PskCategory as Wire;
        match self {
            PskCategory::LongTerm { .. } => Wire::LongTerm,
            PskCategory::Pairing => Wire::Pairing,
            PskCategory::Sentinel => Wire::Sentinel,
        }
    }
}

/// A 32-byte pre-shared key together with its derived identifier.
#[derive(Clone)]
pub struct Psk {
    bytes: [u8; 32],
}

impl std::fmt::Debug for Psk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Psk")
            .field("psk_id", &self.psk_id())
            .finish_non_exhaustive()
    }
}

impl Psk {
    /// Wrap raw PSK bytes.
    pub fn new(bytes: [u8; 32]) -> Self {
        Self { bytes }
    }

    /// Generate a fresh PSK from the OS CSPRNG (used for new pairing records).
    pub fn generate() -> Result<Self> {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(|e| Error::Crypto(format!("CSPRNG failure: {e}")))?;
        Ok(Self { bytes })
    }

    /// The published Sentinel PSK.
    pub fn sentinel() -> Self {
        Self {
            bytes: SENTINEL_PSK,
        }
    }

    /// Parse from the 43-character base64url wire form.
    pub fn from_b64url(s: &str) -> Result<Self> {
        Ok(Self {
            bytes: b64url_decode_32(s)?,
        })
    }

    /// Raw PSK bytes.
    pub fn bytes(&self) -> &[u8; 32] {
        &self.bytes
    }

    /// base64url form (43 chars, no padding).
    pub fn to_b64url(&self) -> String {
        b64url_encode(&self.bytes)
    }

    /// Derive the wire `psk_id`: `base64url(SHA-256("sendspin-psk-id-v1" || PSK))`.
    pub fn psk_id(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(PSK_ID_LABEL);
        hasher.update(self.bytes);
        b64url_encode(&hasher.finalize())
    }
}

/// A PSK candidate the client is willing to use in a handshake, tagged with
/// its category. Built from the sentinel, the pairing config, and the record
/// store; a candidate whose method is disabled must be excluded by the caller.
#[derive(Debug, Clone)]
pub struct PskCandidate {
    /// The PSK itself.
    pub psk: Psk,
    /// The trust category the PSK belongs to.
    pub category: PskCategory,
}

/// Select the candidate matching a wire `psk_id` under the declared
/// `psk_category`, if any. A `psk_id` held only under a different category is
/// a lookup miss.
///
/// A miss in the **initial** handshake falls back to the Sentinel PSK (spec:
/// Sentinel Fallback); a miss during a re-handshake fails the handshake.
pub fn select_psk<'a>(
    candidates: &'a [PskCandidate],
    psk_id: &str,
    category: crate::protocol::messages::PskCategory,
) -> Option<&'a PskCandidate> {
    candidates
        .iter()
        .find(|c| c.category.wire() == category && c.psk.psk_id() == psk_id)
}

// =============================================================================
// Cipher suites
// =============================================================================

/// Noise cipher suite (`<DH>_<cipher>_<hash>` part of the protocol name).
/// Servers must support both; clients pick one in `client/init`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CipherSuite {
    /// `25519_ChaChaPoly_SHA256` - software-friendly suite.
    ChaChaPoly,
    /// `25519_AESGCM_SHA256` - hardware-accelerated suite.
    AesGcm,
}

impl CipherSuite {
    /// The wire identifier used in `client/init`.
    pub fn wire_name(self) -> &'static str {
        match self {
            CipherSuite::ChaChaPoly => "25519_ChaChaPoly_SHA256",
            CipherSuite::AesGcm => "25519_AESGCM_SHA256",
        }
    }

    /// Parse the wire identifier.
    pub fn from_wire_name(s: &str) -> Result<Self> {
        match s {
            "25519_ChaChaPoly_SHA256" => Ok(CipherSuite::ChaChaPoly),
            "25519_AESGCM_SHA256" => Ok(CipherSuite::AesGcm),
            other => Err(Error::Crypto(format!("unknown cipher suite: {other}"))),
        }
    }

    /// Full Noise protocol name for this suite.
    pub fn noise_params(self) -> &'static str {
        match self {
            CipherSuite::ChaChaPoly => "Noise_KKpsk2_25519_ChaChaPoly_SHA256",
            CipherSuite::AesGcm => "Noise_KKpsk2_25519_AESGCM_SHA256",
        }
    }
}

// =============================================================================
// Handshake construction
// =============================================================================

/// Maximum Noise transport message size (Noise spec).
pub const NOISE_MAX_MESSAGE: usize = 65535;

/// AEAD tag size for both defined suites.
pub const AEAD_TAG_LEN: usize = 16;

/// Maximum application payload per non-fragmented frame:
/// 65535 - 16 (AEAD tag) - 1 (message type byte).
pub const MAX_UNFRAGMENTED_PAYLOAD: usize = NOISE_MAX_MESSAGE - AEAD_TAG_LEN - 1;

/// Build the client-side (Noise **responder**) `KKpsk2` handshake state.
///
/// * `prologue` - for the initial handshake, the exact wire bytes of
///   `client/init` followed by `server/init`; for a re-handshake, the prior
///   handshake's hash `h`.
/// * `server_public` - the server's static public key from `server/init`.
/// * `psk` - the PSK selected via the `psk_id` in Noise message 1 (mixed at
///   position 2 per the `psk2` modifier).
pub fn build_client_handshake(
    suite: CipherSuite,
    identity: &Identity,
    server_public: &[u8; 32],
    psk: &Psk,
    prologue: &[u8],
) -> Result<snow::HandshakeState> {
    let params = suite
        .noise_params()
        .parse::<snow::params::NoiseParams>()
        .map_err(|e| Error::Crypto(format!("noise params: {e}")))?;
    snow::Builder::new(params)
        .prologue(prologue)
        .map_err(|e| Error::Crypto(format!("noise prologue: {e}")))?
        .local_private_key(identity.secret_bytes())
        .map_err(|e| Error::Crypto(format!("noise local key: {e}")))?
        .remote_public_key(server_public)
        .map_err(|e| Error::Crypto(format!("noise remote key: {e}")))?
        .psk(2, psk.bytes())
        .map_err(|e| Error::Crypto(format!("noise psk: {e}")))?
        .build_responder()
        .map_err(|e| Error::Crypto(format!("noise responder: {e}")))
}

/// Build the server-side (Noise **initiator**) `KKpsk2` handshake state.
///
/// Provided for completeness and testing; the Sendspin client library acts as
/// the responder in production.
pub fn build_server_handshake(
    suite: CipherSuite,
    identity: &Identity,
    client_public: &[u8; 32],
    psk: &Psk,
    prologue: &[u8],
) -> Result<snow::HandshakeState> {
    let params = suite
        .noise_params()
        .parse::<snow::params::NoiseParams>()
        .map_err(|e| Error::Crypto(format!("noise params: {e}")))?;
    snow::Builder::new(params)
        .prologue(prologue)
        .map_err(|e| Error::Crypto(format!("noise prologue: {e}")))?
        .local_private_key(identity.secret_bytes())
        .map_err(|e| Error::Crypto(format!("noise local key: {e}")))?
        .remote_public_key(client_public)
        .map_err(|e| Error::Crypto(format!("noise remote key: {e}")))?
        .psk(2, psk.bytes())
        .map_err(|e| Error::Crypto(format!("noise psk: {e}")))?
        .build_initiator()
        .map_err(|e| Error::Crypto(format!("noise initiator: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentinel_psk_matches_spec_derivation() {
        // Sentinel PSK = SHA-256("sendspin-sentinel-psk-v1")
        let derived = Sha256::digest(b"sendspin-sentinel-psk-v1");
        assert_eq!(derived.as_slice(), &SENTINEL_PSK);
    }

    #[test]
    fn sentinel_psk_id_matches_published_constant() {
        assert_eq!(Psk::sentinel().psk_id(), SENTINEL_PSK_ID);
        // And the raw psk_id bytes from the spec.
        let raw = b64url_decode_32(SENTINEL_PSK_ID).unwrap();
        assert_eq!(
            raw,
            [
                0x18, 0x5b, 0x15, 0xf6, 0xd2, 0xda, 0x49, 0x09, 0xbd, 0x1d, 0xc1, 0x56, 0xa4, 0xab,
                0x20, 0x61, 0x03, 0xab, 0xef, 0x01, 0x53, 0xbc, 0xd5, 0x2d, 0x92, 0x61, 0x70, 0xb9,
                0x5c, 0xf7, 0xce, 0x8a,
            ]
        );
    }

    #[test]
    fn identity_id_is_43_chars_and_stable() {
        let identity = Identity::generate().unwrap();
        let id = identity.id();
        assert_eq!(id.len(), 43);
        let restored = Identity::from_secret_bytes(*identity.secret_bytes());
        assert_eq!(restored.id(), id);
        assert_eq!(b64url_decode_32(&id).unwrap(), *identity.public_bytes());
    }

    #[test]
    fn client_credentials_round_trip_and_pairing_token_bind_them() {
        let credentials = ClientCredentials::from_parts(
            Identity::from_secret_bytes([1u8; 32]),
            Psk::new([2u8; 32]),
        );
        let bytes = credentials.to_bytes();
        assert_eq!(bytes.len(), ClientCredentials::SERIALIZED_LEN);
        assert_eq!(bytes[0], 1);
        let restored = ClientCredentials::from_bytes(&bytes).unwrap();
        assert_eq!(restored.client_id(), credentials.client_id());
        assert_eq!(
            restored.pairing_psk().psk_id(),
            credentials.pairing_psk().psk_id()
        );
        assert_eq!(restored.pairing_token(), credentials.pairing_token());
    }

    #[test]
    fn client_credentials_reject_wrong_length_and_version() {
        assert!(ClientCredentials::from_bytes(&[1; 64]).is_err());
        let mut bytes = [0u8; ClientCredentials::SERIALIZED_LEN];
        bytes[0] = 2;
        assert!(ClientCredentials::from_bytes(&bytes).is_err());
        bytes[0] = 1;
        bytes[1] = 1;
        assert!(ClientCredentials::from_bytes(&bytes).is_err());
        bytes[1] = 0;
        bytes[33] = 1;
        assert!(ClientCredentials::from_bytes(&bytes).is_err());
    }

    #[test]
    fn generated_client_credentials_are_nonempty() {
        let credentials = ClientCredentials::generate().unwrap();
        assert_ne!(credentials.identity().secret_bytes(), &[0u8; 32]);
        assert_ne!(credentials.pairing_psk().bytes(), &[0u8; 32]);
    }

    #[test]
    fn select_psk_finds_matching_candidate_scoped_by_category() {
        use crate::protocol::messages::PskCategory as Wire;
        let long_term = Psk::new([7u8; 32]);
        let candidates = vec![
            PskCandidate {
                psk: Psk::sentinel(),
                category: PskCategory::Sentinel,
            },
            PskCandidate {
                psk: long_term.clone(),
                category: PskCategory::LongTerm {
                    server_id: "srv".to_string(),
                },
            },
        ];
        let hit = select_psk(&candidates, SENTINEL_PSK_ID, Wire::Sentinel).unwrap();
        assert_eq!(hit.category, PskCategory::Sentinel);
        // A psk_id held only under a different category is a lookup miss.
        assert!(select_psk(&candidates, &long_term.psk_id(), Wire::Pairing).is_none());
        assert!(select_psk(&candidates, &long_term.psk_id(), Wire::LongTerm).is_some());
        assert!(select_psk(
            &candidates,
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            Wire::LongTerm
        )
        .is_none());
    }

    /// Full KKpsk2 round trip between a simulated server (initiator) and the
    /// client (responder), for both cipher suites: handshake completes, the
    /// handshake hashes agree, and transport-mode AEAD works both ways.
    #[test]
    fn kkpsk2_round_trip_both_suites() {
        for suite in [CipherSuite::ChaChaPoly, CipherSuite::AesGcm] {
            let client_identity = Identity::generate().unwrap();
            let server_identity = Identity::generate().unwrap();
            let psk = Psk::sentinel();
            let prologue = b"client-init-bytes-server-init-bytes";

            let mut server = build_server_handshake(
                suite,
                &server_identity,
                client_identity.public_bytes(),
                &psk,
                prologue,
            )
            .unwrap();
            let mut client = build_client_handshake(
                suite,
                &client_identity,
                server_identity.public_bytes(),
                &psk,
                prologue,
            )
            .unwrap();

            let mut buf = [0u8; NOISE_MAX_MESSAGE];
            let mut payload = [0u8; NOISE_MAX_MESSAGE];

            // Noise message 1 (server -> client) carrying a psk_id payload.
            let msg1_payload = format!("{{\"psk_id\":\"{}\"}}", psk.psk_id());
            let len = server
                .write_message(msg1_payload.as_bytes(), &mut buf)
                .unwrap();
            let plen = client.read_message(&buf[..len], &mut payload).unwrap();
            assert_eq!(&payload[..plen], msg1_payload.as_bytes());

            // Noise message 2 (client -> server) carrying the literal `{}`.
            let len = client.write_message(b"{}", &mut buf).unwrap();
            let plen = server.read_message(&buf[..len], &mut payload).unwrap();
            assert_eq!(&payload[..plen], b"{}");

            assert!(server.is_handshake_finished());
            assert!(client.is_handshake_finished());
            assert_eq!(server.get_handshake_hash(), client.get_handshake_hash());

            let mut server = server.into_transport_mode().unwrap();
            let mut client = client.into_transport_mode().unwrap();

            // Server -> client application frame.
            let frame = [
                &[0u8][..],
                br#"{"type":"server/hello","payload":{"name":"s"}}"#,
            ]
            .concat();
            let len = server.write_message(&frame, &mut buf).unwrap();
            assert_eq!(len, frame.len() + AEAD_TAG_LEN);
            let plen = client.read_message(&buf[..len], &mut payload).unwrap();
            assert_eq!(&payload[..plen], frame.as_slice());

            // Client -> server application frame.
            let len = client.write_message(b"\x00{}", &mut buf).unwrap();
            let plen = server.read_message(&buf[..len], &mut payload).unwrap();
            assert_eq!(&payload[..plen], b"\x00{}");

            // Replay/reorder protection: re-reading the same ciphertext fails.
            assert!(server.read_message(&buf[..len], &mut payload).is_err());
        }
    }

    /// A PSK mismatch must fail the handshake at message 2 (psk2: the PSK is
    /// mixed at the end of the second message).
    #[test]
    fn kkpsk2_psk_mismatch_fails_at_message_2() {
        let client_identity = Identity::generate().unwrap();
        let server_identity = Identity::generate().unwrap();
        let prologue = b"prologue";

        let mut server = build_server_handshake(
            CipherSuite::ChaChaPoly,
            &server_identity,
            client_identity.public_bytes(),
            &Psk::new([1u8; 32]),
            prologue,
        )
        .unwrap();
        let mut client = build_client_handshake(
            CipherSuite::ChaChaPoly,
            &client_identity,
            server_identity.public_bytes(),
            &Psk::new([2u8; 32]),
            prologue,
        )
        .unwrap();

        let mut buf = [0u8; NOISE_MAX_MESSAGE];
        let mut payload = [0u8; NOISE_MAX_MESSAGE];

        // Message 1 is decryptable without the PSK mixed in, so it succeeds.
        let len = server.write_message(b"{}", &mut buf).unwrap();
        client.read_message(&buf[..len], &mut payload).unwrap();

        // Message 2 carries the PSK-mixed state; the server must reject it.
        let len = client.write_message(b"{}", &mut buf).unwrap();
        assert!(server.read_message(&buf[..len], &mut payload).is_err());
    }

    /// Tampering with the prologue (init messages) must abort the handshake.
    #[test]
    fn prologue_mismatch_fails_handshake() {
        let client_identity = Identity::generate().unwrap();
        let server_identity = Identity::generate().unwrap();
        let psk = Psk::sentinel();

        let mut server = build_server_handshake(
            CipherSuite::ChaChaPoly,
            &server_identity,
            client_identity.public_bytes(),
            &psk,
            b"prologue-a",
        )
        .unwrap();
        let mut client = build_client_handshake(
            CipherSuite::ChaChaPoly,
            &client_identity,
            server_identity.public_bytes(),
            &psk,
            b"prologue-b",
        )
        .unwrap();

        let mut buf = [0u8; NOISE_MAX_MESSAGE];
        let mut payload = [0u8; NOISE_MAX_MESSAGE];
        let len = server.write_message(b"{}", &mut buf).unwrap();
        assert!(client.read_message(&buf[..len], &mut payload).is_err());
    }

    #[test]
    fn cipher_suite_wire_names_round_trip() {
        for suite in [CipherSuite::ChaChaPoly, CipherSuite::AesGcm] {
            assert_eq!(
                CipherSuite::from_wire_name(suite.wire_name()).unwrap(),
                suite
            );
        }
        assert!(CipherSuite::from_wire_name("25519_Nonsense_SHA256").is_err());
    }
}
