// ABOUTME: Client-side handling of management/* requests (records, pairing config)
// ABOUTME: Pure request handlers over the pairing store and mutable session config

//! Management request handling.
//!
//! Management commands are scoped to connections with `'management'` in their
//! activities (which the admission rules only allow on a long-term Sendspin
//! PSK). Each `management/*` request is answered by exactly one
//! [`ManagementResult`](crate::protocol::messages::ManagementResult); at most one request is in flight per connection, so
//! ordering alone matches replies to requests.
//!
//! The library persists **records** through the [`PairingStore`](crate::protocol::pairing::PairingStore); the other
//! configuration knobs (Pairing PSK, unpaired access, record mode) are held
//! as in-session state here. Applications that need them to survive restarts
//! observe the forwarded management messages and re-provision the builder.

use crate::protocol::crypto::{Psk, PskCandidate, PskCategory};
use crate::protocol::messages::{
    ManagementAddRecord, ManagementRemoveRecord, ManagementResult, ManagementResultCode,
    ManagementSetPairingConfig, RecordEntry,
};
use crate::protocol::pairing::{PairingRecord, PairingStore, StoreError};
use serde_json::json;
use std::sync::Arc;

/// Mutable pairing/management configuration for one session.
pub(crate) struct ManagementState {
    /// The pairing store (records).
    pub store: Arc<dyn PairingStore>,
    /// The client's Pairing PSK, if configured.
    pub pairing_psk: Option<Psk>,
    /// Whether the Pairing PSK method is enabled.
    pub pairing_psk_enabled: bool,
    /// Whether unpaired access is enabled.
    pub unpaired_access: bool,
    /// `record_mode.psk_id`: the shared-PSK record used as the
    /// storage-exhaustion fallback. Always present — the spec requires a
    /// pre-provisioned shared-PSK record as the default.
    pub record_mode: String,
}

fn result(code: ManagementResultCode) -> ManagementResult {
    ManagementResult {
        result: code,
        data: None,
        storage: None,
    }
}

fn store_error_code(e: StoreError) -> ManagementResultCode {
    match e {
        StoreError::AlreadyExists => ManagementResultCode::AlreadyExists,
        StoreError::NotFound => ManagementResultCode::NotFound,
        StoreError::StorageExhausted => ManagementResultCode::StorageExhausted,
        StoreError::Invalid => ManagementResultCode::Invalid,
    }
}

impl ManagementState {
    /// Does a psk_id collide with any candidate PSK across all categories
    /// (sentinel, pairing PSK, records)?
    fn psk_id_known(&self, psk_id: &str) -> bool {
        if Psk::sentinel().psk_id() == psk_id {
            return true;
        }
        if self
            .pairing_psk
            .as_ref()
            .is_some_and(|p| p.psk_id() == psk_id)
        {
            return true;
        }
        self.store.records().iter().any(|r| r.psk_id() == psk_id)
    }

    /// `management/list-records`
    pub fn list_records(&self) -> ManagementResult {
        let records: Vec<RecordEntry> = self
            .store
            .records()
            .iter()
            .map(|r| RecordEntry {
                psk_id: r.psk_id(),
                server_id: r.server_id.clone(),
                used: r.used,
            })
            .collect();
        ManagementResult {
            result: ManagementResultCode::Ok,
            data: Some(json!({ "records": records })),
            storage: None,
        }
    }

    /// `management/add-record`. On success the new candidate is appended to
    /// `candidates` so it participates in later (re-)handshakes.
    pub fn add_record(
        &mut self,
        req: &ManagementAddRecord,
        candidates: &mut Vec<PskCandidate>,
    ) -> ManagementResult {
        let Ok(psk) = Psk::from_b64url(&req.psk) else {
            return result(ManagementResultCode::Invalid);
        };
        if self.psk_id_known(&psk.psk_id()) {
            return result(ManagementResultCode::AlreadyExists);
        }
        let record = PairingRecord {
            psk,
            server_id: req.server_id.clone(),
            used: false,
        };
        let candidate = record.candidate();
        match self.store.add_record(record) {
            Ok(()) => {
                candidates.push(candidate);
                result(ManagementResultCode::Ok)
            }
            Err(e) => result(store_error_code(e)),
        }
    }

    /// `management/remove-record`. Returns `(result, removed_own_record)`;
    /// when the second value is true the caller must, after sending the
    /// response, close the session with goodbye `unauthorized`.
    pub fn remove_record(
        &mut self,
        req: &ManagementRemoveRecord,
        candidates: &mut Vec<PskCandidate>,
        current_psk_id: &str,
    ) -> (ManagementResult, bool) {
        // A record referenced by record_mode cannot be removed.
        if self.record_mode == req.psk_id {
            return (result(ManagementResultCode::Invalid), false);
        }
        match self.store.remove_record(&req.psk_id) {
            Ok(()) => {
                candidates.retain(|c| c.psk.psk_id() != req.psk_id);
                let removed_own = req.psk_id == current_psk_id;
                (result(ManagementResultCode::Ok), removed_own)
            }
            Err(e) => (result(store_error_code(e)), false),
        }
    }

    /// `management/get-pairing-config`. Pairing-code method objects are
    /// absent: this client does not implement the code-based methods.
    pub fn get_pairing_config(&self) -> ManagementResult {
        let data = json!({
            "pairing_psk": { "enabled": self.pairing_psk_enabled },
            "record_mode": { "psk_id": self.record_mode },
            "unpaired_access": { "enabled": self.unpaired_access },
        });
        ManagementResult {
            result: ManagementResultCode::Ok,
            data: Some(data),
            storage: None,
        }
    }

    /// `management/set-pairing-config` (patch semantics). On success the
    /// candidate set is updated to reflect a rotated Pairing PSK.
    pub fn set_pairing_config(
        &mut self,
        req: &ManagementSetPairingConfig,
        candidates: &mut Vec<PskCandidate>,
    ) -> ManagementResult {
        // Fields set on methods this client does not implement are invalid.
        if req.static_pairing_code.is_some() || req.dynamic_pairing_code.is_some() {
            return result(ManagementResultCode::Invalid);
        }

        // Validate everything before applying anything (the request either
        // applies as a whole or is rejected).
        let mut new_pairing_psk: Option<Psk> = None;
        if let Some(patch) = &req.pairing_psk {
            if let Some(psk_str) = &patch.psk {
                let Ok(psk) = Psk::from_b64url(psk_str) else {
                    return result(ManagementResultCode::Invalid);
                };
                // Collision with a candidate PSK in a different category.
                if self.psk_id_known(&psk.psk_id()) {
                    return result(ManagementResultCode::AlreadyExists);
                }
                new_pairing_psk = Some(psk);
            }
            // Enabling the method requires a configured PSK (either already
            // present or supplied in this same request).
            if patch.enabled == Some(true)
                && new_pairing_psk.is_none()
                && self.pairing_psk.is_none()
            {
                return result(ManagementResultCode::Invalid);
            }
        }
        if let Some(record_mode) = &req.record_mode {
            let records = self.store.records();
            let target = records.iter().find(|r| r.psk_id() == record_mode.psk_id);
            match target {
                Some(record) if record.server_id.is_none() => {}
                // Missing or stored-pubkey record: invalid.
                _ => return result(ManagementResultCode::Invalid),
            }
        }

        // Apply.
        if let Some(patch) = &req.pairing_psk {
            if let Some(psk) = new_pairing_psk {
                // Rotation invalidates previously distributed copies.
                candidates.retain(|c| c.category != PskCategory::Pairing);
                candidates.push(PskCandidate {
                    psk: psk.clone(),
                    category: PskCategory::Pairing,
                });
                self.pairing_psk = Some(psk);
            }
            if let Some(enabled) = patch.enabled {
                self.pairing_psk_enabled = enabled;
                if !enabled {
                    candidates.retain(|c| c.category != PskCategory::Pairing);
                } else if let Some(psk) = &self.pairing_psk {
                    if !candidates
                        .iter()
                        .any(|c| c.category == PskCategory::Pairing)
                    {
                        candidates.push(PskCandidate {
                            psk: psk.clone(),
                            category: PskCategory::Pairing,
                        });
                    }
                }
            }
        }
        if let Some(record_mode) = &req.record_mode {
            self.record_mode = record_mode.psk_id.clone();
        }
        if let Some(patch) = &req.unpaired_access {
            if let Some(enabled) = patch.enabled {
                self.unpaired_access = enabled;
            }
        }
        result(ManagementResultCode::Ok)
    }

    /// `management/open-pairing-window`: no pairing-code method is enabled on
    /// this client, so the request is invalid.
    pub fn open_pairing_window(&self) -> ManagementResult {
        result(ManagementResultCode::Invalid)
    }
}

/// The reply for a `management/*` message arriving on a connection without
/// `'management'` in its activities.
pub(crate) fn permission_denied() -> ManagementResult {
    result(ManagementResultCode::PermissionDenied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::crypto::b64url_encode;
    use crate::protocol::messages::{PairingPskConfigPatch, RecordMode, UnpairedAccessPatch};
    use crate::protocol::pairing::MemoryPairingStore;

    /// The pre-provisioned shared-PSK record every client ships with.
    fn preprovisioned() -> PairingRecord {
        PairingRecord {
            psk: Psk::new([0xAA; 32]),
            server_id: None,
            used: false,
        }
    }

    fn state() -> (ManagementState, Vec<PskCandidate>) {
        let record = preprovisioned();
        let record_mode = record.psk_id();
        let store: Arc<dyn PairingStore> = Arc::new(MemoryPairingStore::with_records(vec![record]));
        let state = ManagementState {
            store,
            pairing_psk: Some(Psk::new([9u8; 32])),
            pairing_psk_enabled: true,
            unpaired_access: true,
            record_mode,
        };
        (state, Vec::new())
    }

    #[test]
    fn add_list_remove_records() {
        let (mut state, mut candidates) = state();
        let psk = Psk::new([1u8; 32]);
        let add = ManagementAddRecord {
            psk: psk.to_b64url(),
            server_id: Some("srv".to_string()),
        };
        assert_eq!(
            state.add_record(&add, &mut candidates).result,
            ManagementResultCode::Ok
        );
        assert_eq!(candidates.len(), 1);
        // Duplicate rejected.
        assert_eq!(
            state.add_record(&add, &mut candidates).result,
            ManagementResultCode::AlreadyExists
        );
        // Adding the sentinel PSK is a cross-category collision.
        let sentinel_add = ManagementAddRecord {
            psk: Psk::sentinel().to_b64url(),
            server_id: None,
        };
        assert_eq!(
            state.add_record(&sentinel_add, &mut candidates).result,
            ManagementResultCode::AlreadyExists
        );

        let list = state.list_records();
        assert_eq!(list.result, ManagementResultCode::Ok);
        let records = &list.data.unwrap()["records"];
        // The pre-provisioned shared record plus the one just added.
        assert_eq!(records.as_array().unwrap().len(), 2);
        assert_eq!(records[1]["psk_id"], psk.psk_id());
        assert_eq!(records[1]["used"], false);

        let (res, own) = state.remove_record(
            &ManagementRemoveRecord {
                psk_id: psk.psk_id(),
            },
            &mut candidates,
            "other",
        );
        assert_eq!(res.result, ManagementResultCode::Ok);
        assert!(!own);
        assert!(candidates.is_empty());
        let (res, _) = state.remove_record(
            &ManagementRemoveRecord {
                psk_id: psk.psk_id(),
            },
            &mut candidates,
            "other",
        );
        assert_eq!(res.result, ManagementResultCode::NotFound);
    }

    #[test]
    fn removing_own_record_flags_session_close() {
        let (mut state, mut candidates) = state();
        let psk = Psk::new([2u8; 32]);
        let _ = state
            .add_record(
                &ManagementAddRecord {
                    psk: psk.to_b64url(),
                    server_id: Some("srv".to_string()),
                },
                &mut candidates,
            )
            .result;
        let (res, own) = state.remove_record(
            &ManagementRemoveRecord {
                psk_id: psk.psk_id(),
            },
            &mut candidates,
            &psk.psk_id(),
        );
        assert_eq!(res.result, ManagementResultCode::Ok);
        assert!(own);
    }

    #[test]
    fn record_mode_reference_protection() {
        let (mut state, mut candidates) = state();
        // A shared-PSK record.
        let shared = Psk::new([3u8; 32]);
        let _ = state
            .add_record(
                &ManagementAddRecord {
                    psk: shared.to_b64url(),
                    server_id: None,
                },
                &mut candidates,
            )
            .result;
        // Point record_mode at it.
        let set = ManagementSetPairingConfig {
            record_mode: Some(RecordMode {
                psk_id: shared.psk_id(),
            }),
            ..Default::default()
        };
        assert_eq!(
            state.set_pairing_config(&set, &mut candidates).result,
            ManagementResultCode::Ok
        );
        // Referenced record cannot be removed.
        let (res, _) = state.remove_record(
            &ManagementRemoveRecord {
                psk_id: shared.psk_id(),
            },
            &mut candidates,
            "other",
        );
        assert_eq!(res.result, ManagementResultCode::Invalid);
        // record_mode must reference a shared record: a stored-pubkey one fails.
        let bound = Psk::new([4u8; 32]);
        let _ = state
            .add_record(
                &ManagementAddRecord {
                    psk: bound.to_b64url(),
                    server_id: Some("srv".to_string()),
                },
                &mut candidates,
            )
            .result;
        let set = ManagementSetPairingConfig {
            record_mode: Some(RecordMode {
                psk_id: bound.psk_id(),
            }),
            ..Default::default()
        };
        assert_eq!(
            state.set_pairing_config(&set, &mut candidates).result,
            ManagementResultCode::Invalid
        );
    }

    #[test]
    fn set_pairing_config_patch_semantics() {
        let (mut state, mut candidates) = state();
        candidates.push(PskCandidate {
            psk: state.pairing_psk.clone().unwrap(),
            category: PskCategory::Pairing,
        });

        // Unimplemented method fields are invalid.
        let set = ManagementSetPairingConfig {
            static_pairing_code: Some(Default::default()),
            ..Default::default()
        };
        assert_eq!(
            state.set_pairing_config(&set, &mut candidates).result,
            ManagementResultCode::Invalid
        );

        // Rotate the Pairing PSK.
        let new_psk = Psk::new([7u8; 32]);
        let set = ManagementSetPairingConfig {
            pairing_psk: Some(PairingPskConfigPatch {
                enabled: None,
                psk: Some(new_psk.to_b64url()),
            }),
            ..Default::default()
        };
        assert_eq!(
            state.set_pairing_config(&set, &mut candidates).result,
            ManagementResultCode::Ok
        );
        assert_eq!(
            state.pairing_psk.as_ref().unwrap().psk_id(),
            new_psk.psk_id()
        );
        let pairing_candidates: Vec<_> = candidates
            .iter()
            .filter(|c| c.category == PskCategory::Pairing)
            .collect();
        assert_eq!(pairing_candidates.len(), 1);
        assert_eq!(pairing_candidates[0].psk.psk_id(), new_psk.psk_id());

        // A PSK colliding with the sentinel is already_exists.
        let set = ManagementSetPairingConfig {
            pairing_psk: Some(PairingPskConfigPatch {
                enabled: None,
                psk: Some(b64url_encode(&crate::protocol::crypto::SENTINEL_PSK)),
            }),
            ..Default::default()
        };
        assert_eq!(
            state.set_pairing_config(&set, &mut candidates).result,
            ManagementResultCode::AlreadyExists
        );

        // Disabling removes the candidate.
        let set = ManagementSetPairingConfig {
            pairing_psk: Some(PairingPskConfigPatch {
                enabled: Some(false),
                psk: None,
            }),
            ..Default::default()
        };
        assert_eq!(
            state.set_pairing_config(&set, &mut candidates).result,
            ManagementResultCode::Ok
        );
        assert!(!candidates
            .iter()
            .any(|c| c.category == PskCategory::Pairing));

        // Toggle unpaired access.
        let set = ManagementSetPairingConfig {
            unpaired_access: Some(UnpairedAccessPatch {
                enabled: Some(false),
            }),
            ..Default::default()
        };
        assert_eq!(
            state.set_pairing_config(&set, &mut candidates).result,
            ManagementResultCode::Ok
        );
        assert!(!state.unpaired_access);

        // Config readback: record_mode is always present.
        let get = state.get_pairing_config();
        let data = get.data.unwrap();
        assert_eq!(data["unpaired_access"]["enabled"], false);
        assert_eq!(data["pairing_psk"]["enabled"], false);
        assert_eq!(data["record_mode"]["psk_id"], preprovisioned().psk_id());
        assert!(data.get("static_pairing_code").is_none());
    }

    #[test]
    fn enabling_pairing_psk_without_a_psk_is_invalid() {
        let (mut state, mut candidates) = state();
        state.pairing_psk = None;
        state.pairing_psk_enabled = false;
        let set = ManagementSetPairingConfig {
            pairing_psk: Some(PairingPskConfigPatch {
                enabled: Some(true),
                psk: None,
            }),
            ..Default::default()
        };
        assert_eq!(
            state.set_pairing_config(&set, &mut candidates).result,
            ManagementResultCode::Invalid
        );
        // Supplying the PSK in the same request makes it valid.
        let set = ManagementSetPairingConfig {
            pairing_psk: Some(PairingPskConfigPatch {
                enabled: Some(true),
                psk: Some(Psk::new([5u8; 32]).to_b64url()),
            }),
            ..Default::default()
        };
        assert_eq!(
            state.set_pairing_config(&set, &mut candidates).result,
            ManagementResultCode::Ok
        );
        assert!(state.pairing_psk_enabled);
    }

    #[test]
    fn open_pairing_window_is_invalid_without_code_methods() {
        let (state, _) = state();
        assert_eq!(
            state.open_pairing_window().result,
            ManagementResultCode::Invalid
        );
    }
}
