//! The session states the agent may be in, and the moves between them.
//!
//! The agent owns none of this. Darpana's `engine/capture/lifecycle.py` is the
//! one table — architecture §5.5 plus the signed-off `PAUSED` and `DISCARDED` —
//! and it exports `session-lifecycle.json`, which is vendored next to this
//! module. This file does not restate the table in Rust: it reads the vendored
//! export, so a state the engine does not recognise cannot be entered here
//! even if someone adds one, and a stale export fails loudly at the first move
//! rather than letting the agent improvise.
//!
//! Consent is the same arrangement. The engine decides which ledger reasons
//! refuse a start and exports them; `may_start_recording` answers from that
//! list, so the agent cannot record on a reason the engine would reject.

use std::collections::HashSet;
use std::sync::OnceLock;

/// Darpana's export, vendored at build time.
const VENDORED_JSON: &str = include_str!("session-lifecycle.json");

/// The table as the engine wrote it.
#[derive(Debug)]
pub struct Lifecycle {
    states: HashSet<String>,
    transitions: HashSet<(String, String)>,
    reasons: std::collections::HashMap<String, String>,
    refusals_to_start: std::collections::HashMap<String, String>,
}

/// Why a move or a start was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Neither the engine nor the agent knows this state.
    UnknownState(String),
    /// Both states exist, but the engine's table has no move between them.
    MoveNotAllowed { from: String, to: String },
    /// The ledger's consent reason is one the engine refuses to record on.
    ConsentRefused { reason: String, sentence: String },
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownState(state) => write!(
                f,
                "{state} is not a session state Darpana recognises, so the agent will not enter it"
            ),
            Self::MoveNotAllowed { from, to } => write!(
                f,
                "{from} -> {to} is not a transition this design allows"
            ),
            Self::ConsentRefused { sentence, .. } => write!(f, "{sentence}"),
        }
    }
}

static LIFECYCLE: OnceLock<Lifecycle> = OnceLock::new();

impl Lifecycle {
    /// The engine's table, parsed once.
    ///
    /// Panics if the vendored export is missing or malformed: an agent that
    /// cannot tell which states exist must not record.
    pub fn vendored() -> &'static Self {
        LIFECYCLE.get_or_init(|| {
            let export: serde_json::Value = serde_json::from_str(VENDORED_JSON)
                .expect("the vendored session-lifecycle.json parses");

            let states: HashSet<String> = export["states"]
                .as_array()
                .expect("the export lists states")
                .iter()
                .map(|s| s.as_str().expect("a state is a string").to_string())
                .collect();

            let transitions: HashSet<(String, String)> = export["transitions"]
                .as_array()
                .expect("the export lists transitions")
                .iter()
                .map(|pair| {
                    let pair = pair.as_array().expect("a transition is a pair");
                    (
                        pair[0].as_str().expect("a state is a string").to_string(),
                        pair[1].as_str().expect("a state is a string").to_string(),
                    )
                })
                .collect();

            let as_map = |key: &str| -> std::collections::HashMap<String, String> {
                export[key]
                    .as_object()
                    .unwrap_or_else(|| panic!("the export carries {key}"))
                    .iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            v.as_str().expect("a sentence is a string").to_string(),
                        )
                    })
                    .collect()
            };

            Self {
                states,
                transitions,
                reasons: as_map("reasons"),
                refusals_to_start: as_map("refusals_to_start"),
            }
        })
    }

    /// Every state the engine recognises.
    pub fn states(&self) -> &HashSet<String> {
        &self.states
    }

    /// The state to move to, and why the engine allows the move.
    pub fn next_state(&self, from: &str, to: &str) -> Result<(String, String), Refusal> {
        for state in [from, to] {
            if !self.states.contains(state) {
                return Err(Refusal::UnknownState(state.to_string()));
            }
        }

        let move_ = (from.to_string(), to.to_string());
        if !self.transitions.contains(&move_) {
            return Err(Refusal::MoveNotAllowed {
                from: from.to_string(),
                to: to.to_string(),
            });
        }

        let because = self
            .reasons
            .get(&format!("{from}->{to}"))
            .cloned()
            .unwrap_or_default();
        Ok((to.to_string(), because))
    }

    /// Whether capture may start on this ledger consent reason.
    ///
    /// `granted` is the only reason that records. Anything else returns the
    /// engine's own sentence, so the coach reads the same refusal whether it
    /// came from the agent or from Darpana.
    pub fn may_start_recording(&self, reason: &str) -> Result<(), Refusal> {
        match self.refusals_to_start.get(reason) {
            None if reason == "granted" => Ok(()),
            None => Err(Refusal::ConsentRefused {
                reason: reason.to_string(),
                sentence: format!(
                    "Recording cannot start: Darpana does not recognise the consent state \
                     '{reason}', so there is no evidence both parties agreed."
                ),
            }),
            Some(sentence) => Err(Refusal::ConsentRefused {
                reason: reason.to_string(),
                sentence: sentence.clone(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_the_engines_table_not_a_copy() {
        let lifecycle = Lifecycle::vendored();

        // The states the architecture names, read back out of the export.
        for state in [
            "recording",
            "paused",
            "uploading",
            "queued",
            "processing",
            "committed",
            "aborted",
            "discarded",
            "raw_expired",
            "raw_revoked",
            "severed",
        ] {
            assert!(
                lifecycle.states().contains(state),
                "the engine no longer exports {state}; the agent is stale"
            );
        }
    }

    #[test]
    fn every_move_the_engine_allows_carries_its_reason() {
        let lifecycle = Lifecycle::vendored();

        for (from, to) in [
            ("recording", "paused"),
            ("paused", "recording"),
            ("recording", "uploading"),
            ("uploading", "queued"),
            ("queued", "processing"),
            ("processing", "committed"),
            ("recording", "discarded"),
        ] {
            let (next, because) = lifecycle
                .next_state(from, to)
                .unwrap_or_else(|e| panic!("{from} -> {to} should be allowed: {e}"));
            assert_eq!(next, to);
            assert!(!because.is_empty(), "{from} -> {to} has no reason");
        }
    }

    #[test]
    fn a_move_outside_the_table_is_refused() {
        let lifecycle = Lifecycle::vendored();

        assert_eq!(
            lifecycle.next_state("committed", "discarded"),
            Err(Refusal::MoveNotAllowed {
                from: "committed".into(),
                to: "discarded".into()
            }),
            "a committed session may not be discarded; severance is the only removal"
        );
        assert_eq!(
            lifecycle.next_state("recording", "committed"),
            Err(Refusal::MoveNotAllowed {
                from: "recording".into(),
                to: "committed".into()
            })
        );
    }

    #[test]
    fn a_state_the_engine_does_not_know_cannot_be_entered() {
        let lifecycle = Lifecycle::vendored();

        assert_eq!(
            lifecycle.next_state("recording", "retrying"),
            Err(Refusal::UnknownState("retrying".into()))
        );
    }

    #[test]
    fn recording_starts_only_on_live_consent_and_in_the_engines_words() {
        let lifecycle = Lifecycle::vendored();

        assert!(lifecycle.may_start_recording("granted").is_ok());

        let engine: serde_json::Value = serde_json::from_str(VENDORED_JSON).unwrap();
        for (reason, sentence) in engine["refusals_to_start"].as_object().unwrap() {
            let refusal = lifecycle
                .may_start_recording(reason)
                .expect_err("a non-granted reason must refuse");
            assert_eq!(
                refusal.to_string(),
                sentence.as_str().unwrap(),
                "{reason}: the agent must say what the engine says"
            );
        }
    }

    #[test]
    fn an_unrecognised_consent_reason_refuses_rather_than_records() {
        let lifecycle = Lifecycle::vendored();

        assert!(
            lifecycle.may_start_recording("probably_fine").is_err(),
            "a reason the engine never exported must fail closed"
        );
    }
}
