// ABOUTME: Session-level protocol state machine: activate admissibility, pairing,
// ABOUTME: and in-band re-handshakes over an established channel

//! Session rules and the session state machine.
//!
//! The pure functions at the top implement the spec's `server/activate`
//! admissibility table (activity sets constrained by the matched PSK
//! category, playback-capability). `SessionState` applies them — together
//! with pairing and re-handshake handling — to the message stream of an
//! established connection. The I/O loop itself (frame decode, channel
//! routing, task lifecycle) lives in [`client`](crate::protocol::client);
//! this module owns the protocol decisions.

use crate::error::Error;
use crate::protocol::crypto::{
    b64url_decode, b64url_encode, select_psk, CipherSuite, Identity, Psk, PskCandidate, PskCategory,
};
use crate::protocol::messages::{
    Activity, ClientGoodbye, ClientHello, ClientPairFinalize, DeviceInfo, GoodbyeReason, Message,
    NoiseHandshake, NoiseMessage1Payload, PairAbort, PairAbortReason, PairingMethod,
    ServerActivate, SupportedPairMethods, UnpairedAccess,
};
use crate::protocol::pairing::{PairingRecord, PairingStore};
use crate::protocol::roles::SharedSessionState;
use crate::protocol::transport::{ClientHandshake, EncryptedChannel};
use crate::protocol::writer::{OutboundPayload, WriteCommand};
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::watch;
use tokio::time::Instant;

/// Recommended bound on a pairing attempt, measured from its first pairing
/// message (spec: Entering and leaving pairing).
pub(crate) const PAIRING_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(120);

/// How the client must respond to a non-admissible `server/activate`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ActivateVerdict {
    /// The activation is admissible.
    Admissible,
    /// Close with `client/goodbye` reason `pairing_required`: enabling
    /// unpaired access would have made this activation admissible.
    PairingRequired,
    /// Close with `client/goodbye` reason `unauthorized`.
    Unauthorized,
    /// Reply with `pair/abort` reason `method_not_supported`, leaving the
    /// connection open.
    MethodNotSupported,
}

/// Is `activities` an allowed set for the matched PSK category?
fn is_allowed_set(category: &PskCategory, activities: &[Activity], unpaired_access: bool) -> bool {
    let has = |a: Activity| activities.contains(&a);
    let only_pairing = activities.len() == 1 && has(Activity::Pairing);
    let only_playback = activities.len() == 1 && has(Activity::Playback);
    match category {
        // `[]` or `['playback']`
        PskCategory::LongTerm { .. } => activities.is_empty() || only_playback,
        // exactly `['pairing']`
        PskCategory::Pairing => only_pairing,
        // `[]`, `['pairing']`, `['playback']` (the latter only with unpaired access)
        PskCategory::Sentinel => {
            activities.is_empty() || only_pairing || (unpaired_access && only_playback)
        }
    }
}

/// Is the connection *playback-capable*: its activities extended with
/// `'playback'` are an allowed set for the matched PSK?
fn is_playback_capable(
    category: &PskCategory,
    activities: &[Activity],
    unpaired_access: bool,
) -> bool {
    let mut extended: Vec<Activity> = activities.to_vec();
    if !extended.contains(&Activity::Playback) {
        extended.push(Activity::Playback);
    }
    is_allowed_set(category, &extended, unpaired_access)
}

/// Evaluate a `server/activate` against the session's matched PSK category.
///
/// `effective_roles` is the message's `active_roles` if present, otherwise the
/// persisted roles from earlier activations (spec: the field persists across
/// `server/activate` messages that omit it; on the first activation an omitted
/// field is treated as empty). Note the special persisted-roles rule: when a
/// later activation makes the connection non-playback-capable without
/// explicitly sending `active_roles`, the persisted roles are treated as empty
/// rather than the message rejected — the caller expresses that by passing the
/// roles it would persist.
///
/// `pairing_method_ok` reports whether the `pairing` object (when `'pairing'`
/// is declared) names a method/format the client currently offers and the
/// matched PSK permits; the pairing subsystem computes it.
pub(crate) fn evaluate_activate(
    category: &PskCategory,
    unpaired_access: bool,
    activate: &ServerActivate,
    effective_roles: &[String],
    explicit_roles: bool,
    pairing_method_ok: bool,
) -> ActivateVerdict {
    let activities = &activate.activities;
    let allowed = is_allowed_set(category, activities, unpaired_access);
    let playback_capable = is_playback_capable(category, activities, unpaired_access);

    // Persisted (non-explicit) roles on a no-longer-playback-capable
    // connection degrade to empty instead of rejecting the message.
    let roles: &[String] = if !explicit_roles && !playback_capable {
        &[]
    } else {
        effective_roles
    };

    let roles_violation = !roles.is_empty() && !playback_capable;

    if !allowed || roles_violation {
        // `pairing_required` applies when the matched PSK is the Sentinel,
        // unpaired access is disabled, and enabling it would make the
        // activation admissible.
        if *category == PskCategory::Sentinel && !unpaired_access {
            let allowed_hyp = is_allowed_set(category, activities, true);
            let capable_hyp = is_playback_capable(category, activities, true);
            let roles_hyp: &[String] = if !explicit_roles && !capable_hyp {
                &[]
            } else {
                effective_roles
            };
            let roles_violation_hyp = !roles_hyp.is_empty() && !capable_hyp;
            if allowed_hyp && !roles_violation_hyp {
                return ActivateVerdict::PairingRequired;
            }
        }
        return ActivateVerdict::Unauthorized;
    }

    if activities.contains(&Activity::Pairing) && !pairing_method_ok {
        return ActivateVerdict::MethodNotSupported;
    }

    ActivateVerdict::Admissible
}

/// Rank of an activity set for multi-server admission arbitration:
/// `playback > pairing > empty`.
pub(crate) fn activity_rank(activities: &[Activity]) -> u8 {
    if activities.contains(&Activity::Playback) {
        2
    } else if activities.contains(&Activity::Pairing) {
        1
    } else {
        0
    }
}

/// Does the activation's pairing object name a method/format this client
/// offers and the matched PSK permits? Only meaningful when `'pairing'` is in
/// the activity set.
pub(crate) fn pairing_method_ok(
    activate: &ServerActivate,
    category: &PskCategory,
    supported: &SupportedPairMethods,
) -> bool {
    if !activate.activities.contains(&Activity::Pairing) {
        return true;
    }
    let Some(pairing) = activate.pairing.as_ref() else {
        return false;
    };
    // `pairing.method` MUST be `pairing_psk` iff the matched PSK is the
    // pairing PSK.
    let is_pairing_psk_session = *category == PskCategory::Pairing;
    if (pairing.method == PairingMethod::PairingPsk) != is_pairing_psk_session {
        return false;
    }
    match pairing.method {
        PairingMethod::PairingPsk => supported.pairing_psk.is_some(),
        PairingMethod::StaticPairingCode => supported.static_pairing_code.is_some(),
        PairingMethod::DynamicPairingCode => {
            let Some(descriptor) = supported.dynamic_pairing_code.as_ref() else {
                return false;
            };
            let Some(format) = pairing.format else {
                return false;
            };
            descriptor.formats.contains(&format)
        }
    }
}

/// The connection-independent parts of `client/hello`.
#[derive(Clone)]
pub(crate) struct HelloTemplate {
    pub name: String,
    pub device_info: Option<DeviceInfo>,
    pub supported_roles: Vec<String>,
    pub player_v1_support: Option<crate::protocol::messages::PlayerV1Support>,
    pub source_v1_support: Option<crate::protocol::messages::SourceV1Support>,
    pub visualizer_v1_support: Option<crate::protocol::messages::VisualizerV1Support>,
    pub supported_pair_methods: SupportedPairMethods,
}

impl HelloTemplate {
    /// Build the `client/hello` for the given per-connection facts.
    pub(crate) fn to_hello(&self, unpaired_access: bool) -> ClientHello {
        ClientHello {
            name: self.name.clone(),
            device_info: self.device_info.clone(),
            supported_roles: self.supported_roles.clone(),
            player_v1_support: self.player_v1_support.clone(),
            source_v1_support: self.source_v1_support.clone(),
            visualizer_v1_support: self.visualizer_v1_support.clone(),
            supported_pair_methods: self.supported_pair_methods.clone(),
            unpaired_access: UnpairedAccess {
                enabled: unpaired_access,
            },
        }
    }
}

fn finish_rehandshake(rehandshake_in_progress: &mut bool, gate: &watch::Sender<bool>) {
    *rehandshake_in_progress = false;
    let _ = gate.send(true);
}

/// Fire-and-forget enqueue of a JSON message toward the writer task.
pub(crate) fn enqueue_json(out_tx: &UnboundedSender<WriteCommand>, msg: Message) {
    let (ack, _rx) = tokio::sync::oneshot::channel();
    let _ = out_tx.send(WriteCommand::Send {
        msg: OutboundPayload::Json(Box::new(msg)),
        ack,
    });
}

/// Fire-and-forget enqueue of a farewell (goodbye/pair-abort + close).
fn enqueue_farewell(out_tx: &UnboundedSender<WriteCommand>, msg: Message) {
    let (ack, _rx) = tokio::sync::oneshot::channel();
    let _ = out_tx.send(WriteCommand::Farewell {
        msg: Box::new(msg),
        ack,
    });
}

/// I/O handles the session state machine acts through.
pub(crate) struct SessionIo {
    /// The live encrypted channel (shared with the writer task).
    pub channel: Arc<Mutex<EncryptedChannel>>,
    /// The writer task's queue.
    pub out_tx: UnboundedSender<WriteCommand>,
    /// Write gate for application senders: closed during a re-handshake,
    /// reopened when the post-re-handshake `server/activate` arrives.
    pub gate: watch::Sender<bool>,
}

/// What the router should do with a message after the state machine ran.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SessionFlow {
    /// Forward the message to the application channel.
    Forward,
    /// Internal protocol traffic; do not forward.
    Consumed,
    /// The session is over; stop the router loop.
    Close,
}

/// Mutable protocol state of one established session.
pub(crate) struct SessionState {
    pub identity: Arc<Identity>,
    pub suite: CipherSuite,
    pub server_public: [u8; 32],
    pub server_id: String,
    pub hello: HelloTemplate,
    /// The pairing record store.
    pub store: Arc<dyn PairingStore>,
    /// Whether unpaired access is enabled.
    pub unpaired_access: bool,
    /// Live PSK candidate set (grows when a pairing record is persisted).
    pub candidates: Vec<PskCandidate>,
    /// The candidate that authenticated the current key set.
    pub current: PskCandidate,
    /// Long-term PSK delivered in an in-flight Pairing PSK attempt.
    pub pending_pairing: Option<Psk>,
    /// Deadline of the in-flight pairing attempt (spec attempt timeout).
    pub pairing_deadline: Option<Instant>,
    /// Activities from the most recent admissible server/activate.
    pub activities: Vec<Activity>,
    /// Persisted `active_roles` (spec: the field persists when omitted).
    pub persisted_roles: Vec<String>,
    /// Whether client/state has been sent under the current key set.
    pub state_sent: bool,
    /// Reject a second re-handshake until the writer has applied the first key
    /// swap and the server has completed its activate sequence.
    pub rehandshake_in_progress: bool,
    /// Live state shared with the message router and application handles.
    pub shared: Arc<SharedSessionState>,
}

impl SessionState {
    /// Handle one decrypted JSON protocol message.
    pub(crate) fn handle_json(&mut self, msg: &Message, io: &SessionIo) -> SessionFlow {
        match msg {
            Message::StreamStart(start) => {
                // Settle the live stream state before forwarding, so a
                // consumer reacting to stream/start sees current state.
                self.shared.stream().note_stream_start(start);
                SessionFlow::Forward
            }
            Message::StreamEnd(end) => {
                self.shared.stream().note_stream_end(end);
                SessionFlow::Forward
            }
            Message::ServerActivate(activate) => self.handle_activate(activate, io),
            Message::ServerPairFinalize(_) => self.handle_pair_finalize(),
            Message::PairAbort(abort) => {
                log::info!("Pairing aborted: {:?}", abort.reason);
                self.clear_pairing_attempt();
                SessionFlow::Forward
            }
            Message::ServerUnpair(_) => self.handle_unpair(io),
            Message::NoiseHandshake(nh) => self.handle_rehandshake(nh, io),
            Message::ServerHello(_) => {
                // A server/hello mid-session follows a re-handshake:
                // answer with a fresh client/hello.
                enqueue_json(
                    &io.out_tx,
                    Message::ClientHello(self.hello.to_hello(self.unpaired_access)),
                );
                SessionFlow::Forward
            }
            _ => SessionFlow::Forward,
        }
    }

    fn handle_activate(&mut self, activate: &ServerActivate, io: &SessionIo) -> SessionFlow {
        let explicit = activate.active_roles.is_some();
        let roles = activate
            .active_roles
            .clone()
            .unwrap_or_else(|| self.persisted_roles.clone());
        let pairing_ok = pairing_method_ok(
            activate,
            &self.current.category,
            &self.hello.supported_pair_methods,
        );
        match evaluate_activate(
            &self.current.category,
            self.unpaired_access,
            activate,
            &roles,
            explicit,
            pairing_ok,
        ) {
            ActivateVerdict::Admissible => {
                let roles_grew = roles.iter().any(|r| !self.persisted_roles.contains(r));
                self.persisted_roles = roles.clone();
                self.activities = activate.activities.clone();
                self.shared.set_activation(self.activities.clone(), roles);
                // The activate ends any in-flight re-handshake sequence;
                // reopen the write gate for application senders.
                finish_rehandshake(&mut self.rehandshake_in_progress, &io.gate);
                let pairing_now = activate.activities.contains(&Activity::Pairing);
                // Spec: when a role becomes active, the client must send a
                // client/state that includes that role's object (the server
                // sends no binary data until it has it); also (re)send after
                // a key rotation. The canonical state carries every role's
                // full object, so re-sending it covers both.
                if !pairing_now && (!self.state_sent || roles_grew) {
                    enqueue_json(&io.out_tx, Message::ClientState(self.shared.client_state()));
                    self.state_sent = true;
                }
                if pairing_now {
                    // Pairing PSK flow: start (or supersede) the attempt.
                    if self.current.category == PskCategory::Pairing {
                        self.start_pairing_attempt(io);
                    }
                } else if self.pending_pairing.is_some() {
                    // Cancelling activate: the attempt ends, nothing is
                    // persisted.
                    log::info!("Pairing attempt cancelled by server/activate");
                    self.clear_pairing_attempt();
                }
                SessionFlow::Forward
            }
            ActivateVerdict::PairingRequired => {
                enqueue_farewell(
                    &io.out_tx,
                    Message::ClientGoodbye(ClientGoodbye {
                        reason: GoodbyeReason::PairingRequired,
                    }),
                );
                SessionFlow::Close
            }
            ActivateVerdict::Unauthorized => {
                enqueue_farewell(
                    &io.out_tx,
                    Message::ClientGoodbye(ClientGoodbye {
                        reason: GoodbyeReason::Unauthorized,
                    }),
                );
                SessionFlow::Close
            }
            ActivateVerdict::MethodNotSupported => {
                // This terminal activation still ends the re-handshake sequence;
                // the pairing attempt itself was rejected, but the connection
                // remains usable for ordinary traffic.
                finish_rehandshake(&mut self.rehandshake_in_progress, &io.gate);
                enqueue_json(
                    &io.out_tx,
                    Message::PairAbort(PairAbort {
                        reason: PairAbortReason::MethodNotSupported,
                    }),
                );
                SessionFlow::Forward
            }
        }
    }

    fn handle_pair_finalize(&mut self) -> SessionFlow {
        // Server persisted its record; now persist ours (Pairing PSK flow).
        let Some(psk) = self.pending_pairing.take() else {
            log::warn!("server/pair-finalize with no pairing in flight");
            return SessionFlow::Forward;
        };
        self.clear_pairing_attempt();
        let record = PairingRecord {
            psk,
            server_id: self.server_id.clone(),
            used: false,
        };
        let candidate = record.candidate();
        match self.store.add_record(record) {
            Ok(()) => {
                log::info!("Pairing record persisted for {}", self.server_id);
                // A re-pair replaces the prior record for this server; keep
                // the live candidate set in sync with the store as well.
                self.candidates.retain(|existing| {
                    !matches!(
                        &existing.category,
                        PskCategory::LongTerm { server_id } if server_id == &self.server_id
                    )
                });
                self.candidates.push(candidate);
                SessionFlow::Forward
            }
            Err(e) => {
                // The server believes pairing completed and will re-handshake
                // to the new PSK, which this client cannot honor. Failing the
                // session now surfaces the storage problem instead of an
                // opaque re-handshake failure later.
                log::error!("Failed to persist pairing record: {e:?}; closing session");
                SessionFlow::Close
            }
        }
    }

    fn handle_unpair(&mut self, io: &SessionIo) -> SessionFlow {
        match &self.current.category {
            PskCategory::LongTerm { .. } => {
                let psk_id = self.current.psk.psk_id();
                let _ = self.store.remove_record(&psk_id);
                self.candidates.retain(|c| c.psk.psk_id() != psk_id);
                enqueue_farewell(
                    &io.out_tx,
                    Message::ClientGoodbye(ClientGoodbye {
                        reason: GoodbyeReason::Unpaired,
                    }),
                );
                SessionFlow::Close
            }
            // Unpaired session: there is no record to remove; ignore.
            _ => SessionFlow::Consumed,
        }
    }

    /// In-band re-handshake (e.g. after pairing, or key rotation): process
    /// Noise message 1 (delivered as an encrypted `noise/handshake` JSON
    /// message), select the PSK, and enqueue message 2 plus the key swap on
    /// the writer. The prologue is the prior handshake's hash `h`;
    /// `client/init` / `server/init` are not re-sent.
    ///
    /// Spec: no other messages flow during the exchange, and afterwards the
    /// connection resumes with `server/hello` → `client/hello` →
    /// `server/activate` — so the write gate closes here and reopens when
    /// that `server/activate` arrives.
    fn handle_rehandshake(&mut self, msg1: &NoiseHandshake, io: &SessionIo) -> SessionFlow {
        if self.rehandshake_in_progress {
            log::error!("concurrent re-handshake received; closing session");
            return SessionFlow::Close;
        }
        let result = (|| -> Result<PskCandidate, Error> {
            let prologue = *io.channel.lock().handshake_hash();
            let mut handshake =
                ClientHandshake::new(self.suite, &self.identity, &self.server_public, &prologue)?;
            let msg1_bytes = b64url_decode(&msg1.data)?;
            let payload = handshake.read_message_1(&msg1_bytes)?;
            let psk_payload: NoiseMessage1Payload = serde_json::from_slice(&payload)
                .map_err(|e| Error::Protocol(format!("malformed noise message 1 payload: {e}")))?;
            // The Sentinel Fallback applies only to the initial handshake; a
            // lookup miss during a re-handshake fails the handshake.
            let candidate = select_psk(
                &self.candidates,
                &psk_payload.psk_id,
                psk_payload.psk_category,
            )
            .ok_or_else(|| Error::Crypto("psk_id lookup miss on re-handshake".to_string()))?
            .clone();
            if let PskCategory::LongTerm { server_id: bound } = &candidate.category {
                if *bound != self.server_id {
                    return Err(Error::Crypto(
                        "matched PSK is bound to a different server_id".to_string(),
                    ));
                }
            }
            let msg2_bytes = handshake.write_message_2(&candidate.psk)?;
            let new_channel = handshake.into_channel()?;
            let msg2 = Message::NoiseHandshake(NoiseHandshake {
                data: b64url_encode(&msg2_bytes),
            });
            // Gate application senders before enqueueing the key swap so no
            // new traffic interleaves with the re-handshake sequence.
            let _ = io.gate.send(false);
            io.out_tx
                .send(WriteCommand::Rehandshake {
                    msg2: Box::new(msg2),
                    new_channel: Box::new(new_channel),
                })
                .map_err(|_| Error::WebSocket("connection closed".to_string()))?;
            self.rehandshake_in_progress = true;
            Ok(candidate)
        })();

        match result {
            Ok(new_current) => {
                log::info!(
                    "Re-handshake to psk category {:?} initiated",
                    new_current.category
                );
                if matches!(new_current.category, PskCategory::LongTerm { .. }) {
                    self.store.mark_used(&new_current.psk.psk_id());
                }
                self.current = new_current;
                self.clear_pairing_attempt();
                self.persisted_roles = Vec::new();
                self.shared
                    .set_activation(self.activities.clone(), Vec::new());
                self.state_sent = false;
                SessionFlow::Consumed
            }
            Err(e) => {
                log::error!("Re-handshake failed: {e}");
                SessionFlow::Close
            }
        }
    }

    /// Begin (or supersede) a Pairing PSK attempt: deliver a freshly
    /// generated long-term PSK and start the attempt timeout.
    pub(crate) fn start_pairing_attempt(&mut self, io: &SessionIo) {
        match Psk::generate() {
            Ok(new_psk) => {
                log::info!("Starting Pairing PSK flow: sending client/pair-finalize");
                enqueue_json(
                    &io.out_tx,
                    Message::ClientPairFinalize(ClientPairFinalize {
                        long_term_psk: Some(new_psk.to_b64url()),
                        wrapped_psk: None,
                    }),
                );
                self.pending_pairing = Some(new_psk);
                self.pairing_deadline = Some(Instant::now() + PAIRING_ATTEMPT_TIMEOUT);
                self.shared.set_pairing_attempt(true);
            }
            Err(e) => log::error!("PSK generation failed: {e}"),
        }
    }

    /// The attempt timeout elapsed: abort the attempt (spec reason
    /// `attempt_timeout`) and discard its state. The connection stays open.
    pub(crate) fn abort_pairing_attempt_timeout(&mut self, io: &SessionIo) {
        log::warn!("Pairing attempt timed out; sending pair/abort");
        enqueue_json(
            &io.out_tx,
            Message::PairAbort(PairAbort {
                reason: PairAbortReason::AttemptTimeout,
            }),
        );
        self.clear_pairing_attempt();
    }

    fn clear_pairing_attempt(&mut self) {
        self.pending_pairing = None;
        self.pairing_deadline = None;
        self.shared.set_pairing_attempt(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn activate(activities: Vec<Activity>, roles: Option<Vec<&str>>) -> ServerActivate {
        ServerActivate {
            activities,
            active_roles: roles.map(|r| r.iter().map(|s| s.to_string()).collect()),
            pairing: None,
        }
    }

    fn eval(
        category: PskCategory,
        unpaired: bool,
        act: &ServerActivate,
        pairing_ok: bool,
    ) -> ActivateVerdict {
        let roles = act.active_roles.clone().unwrap_or_default();
        evaluate_activate(
            &category,
            unpaired,
            act,
            &roles,
            act.active_roles.is_some(),
            pairing_ok,
        )
    }

    fn lt() -> PskCategory {
        PskCategory::LongTerm {
            server_id: "srv".to_string(),
        }
    }

    #[test]
    fn long_term_allows_empty_or_playback_only() {
        for acts in [vec![], vec![Activity::Playback]] {
            let a = activate(acts, Some(vec!["player@v1"]));
            assert_eq!(eval(lt(), false, &a, true), ActivateVerdict::Admissible);
        }
        // Pairing is not an allowed set on a long-term PSK (the server
        // re-handshakes to the pairing PSK first).
        let a = activate(vec![Activity::Pairing], Some(vec![]));
        assert_eq!(eval(lt(), false, &a, true), ActivateVerdict::Unauthorized);
        // pairing mixed with playback is not an allowed set
        let a = activate(vec![Activity::Pairing, Activity::Playback], None);
        assert_eq!(eval(lt(), false, &a, true), ActivateVerdict::Unauthorized);
    }

    #[test]
    fn pairing_psk_allows_only_pairing() {
        let cat = PskCategory::Pairing;
        let ok = activate(vec![Activity::Pairing], Some(vec![]));
        assert_eq!(
            eval(cat.clone(), false, &ok, true),
            ActivateVerdict::Admissible
        );
        let bad = activate(vec![Activity::Playback], None);
        assert_eq!(eval(cat, false, &bad, true), ActivateVerdict::Unauthorized);
    }

    /// Spec worked example: Sentinel + unpaired access disabled + playback →
    /// pairing_required; pairing with non-empty active_roles → unauthorized.
    #[test]
    fn sentinel_worked_example() {
        let a = activate(vec![Activity::Playback], Some(vec!["player@v1"]));
        assert_eq!(
            eval(PskCategory::Sentinel, false, &a, true),
            ActivateVerdict::PairingRequired
        );
        let b = activate(vec![Activity::Pairing], Some(vec!["player@v1"]));
        assert_eq!(
            eval(PskCategory::Sentinel, false, &b, true),
            ActivateVerdict::Unauthorized
        );
    }

    #[test]
    fn sentinel_with_unpaired_access_admits_playback() {
        let a = activate(vec![Activity::Playback], Some(vec!["player@v1"]));
        assert_eq!(
            eval(PskCategory::Sentinel, true, &a, true),
            ActivateVerdict::Admissible
        );
    }

    #[test]
    fn sentinel_empty_set_is_admissible_without_roles() {
        let a = activate(vec![], Some(vec![]));
        assert_eq!(
            eval(PskCategory::Sentinel, false, &a, true),
            ActivateVerdict::Admissible
        );
    }

    #[test]
    fn source_role_admitted_on_unpaired_access() {
        // source@v1 activation is a server-side approval concern; the client
        // admits it under the ordinary playback-capability rules.
        let a = activate(
            vec![Activity::Playback],
            Some(vec!["player@v1", "source@v1"]),
        );
        assert_eq!(
            eval(PskCategory::Sentinel, true, &a, true),
            ActivateVerdict::Admissible
        );
        assert_eq!(eval(lt(), false, &a, true), ActivateVerdict::Admissible);
    }

    #[test]
    fn empty_activities_may_carry_roles_when_playback_capable() {
        // Long-term PSK, empty activities: still playback-capable, so a
        // non-empty active_roles is admissible.
        let a = activate(vec![], Some(vec!["player@v1"]));
        assert_eq!(eval(lt(), false, &a, true), ActivateVerdict::Admissible);
    }

    #[test]
    fn persisted_roles_degrade_when_not_playback_capable() {
        // Pairing activation omitting active_roles with persisted roles:
        // treated as empty, not rejected.
        let a = activate(vec![Activity::Pairing], None);
        let persisted = vec!["player@v1".to_string()];
        let verdict = evaluate_activate(&PskCategory::Pairing, false, &a, &persisted, false, true);
        assert_eq!(verdict, ActivateVerdict::Admissible);
    }

    #[test]
    fn pairing_method_mismatch_aborts_without_closing() {
        let a = activate(vec![Activity::Pairing], Some(vec![]));
        assert_eq!(
            eval(PskCategory::Pairing, false, &a, false),
            ActivateVerdict::MethodNotSupported
        );
    }

    #[test]
    fn finish_rehandshake_clears_guard_and_opens_gate() {
        let (gate, mut observed) = watch::channel(false);
        let mut in_progress = true;
        finish_rehandshake(&mut in_progress, &gate);
        assert!(!in_progress);
        assert!(*observed.borrow_and_update());
    }

    #[test]
    fn activity_ranking_orders_playback_pairing_empty() {
        assert!(activity_rank(&[Activity::Playback]) > activity_rank(&[Activity::Pairing]));
        assert!(activity_rank(&[Activity::Pairing]) > activity_rank(&[]));
        assert_eq!(
            activity_rank(&[Activity::Pairing, Activity::Playback]),
            activity_rank(&[Activity::Playback])
        );
    }
}
