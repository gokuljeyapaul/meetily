//! What the agent does about who else is in the call.
//!
//! The agent polls the meeting platform; it does not decide what a reading
//! means. Darpana's `engine/capture/roster.py` is the policy and it is
//! fail-closed by the owner's decision: a platform that exposes no roster
//! refuses to record at all, a third participant or a poll the agent could not
//! complete pauses capture, and a pause lifts only when both parties consent
//! again — not when the third person leaves.
//!
//! `decide` is four comparisons, so it is reimplemented here rather than
//! called over a wire; the wording and the two bounds are vendored from
//! `capture-roster.json`, and the test below fails if the engine rewords them.

use std::time::Duration;

/// Darpana's export, vendored at build time.
const VENDORED_JSON: &str = include_str!("capture-roster.json");

/// How many polls may fail before capture pauses: none.
pub const MISSED_POLL_LIMIT: u32 = 1;

/// How long capture may continue after a roster change before it must pause.
pub const PAUSE_WITHIN: Duration = Duration::from_millis(5_000);

/// What the agent does with the reading it just took.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Both parties and nobody else: keep capturing.
    Record,
    /// Stop capturing but keep the session; only re-consent resumes it.
    Pause,
    /// Do not capture at all on this platform.
    Refuse,
}

impl Decision {
    /// The engine's sentence for this decision, for the coach to read.
    pub fn sentence(self) -> &'static str {
        let key = match self {
            Self::Record => "record",
            Self::Pause => "pause",
            Self::Refuse => "refuse",
        };
        sentences()
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| *v)
            .expect("the vendored export carries every decision's sentence")
    }
}

/// One poll of the meeting platform's participant list.
#[derive(Debug, Clone, Copy)]
pub struct RosterReading {
    /// How many participants the platform reported.
    pub participants: u32,
    /// Whether this platform exposes a participant list at all.
    pub roster_supported: bool,
    /// How many consecutive polls the agent failed to complete.
    pub missed_polls: u32,
    /// Whether capture is currently paused.
    pub was_paused: bool,
    /// Whether both parties have consented again since the pause.
    pub reconsented: bool,
}

impl Default for RosterReading {
    fn default() -> Self {
        Self {
            participants: 2,
            roster_supported: true,
            missed_polls: 0,
            was_paused: false,
            reconsented: false,
        }
    }
}

/// The engine's policy, in the engine's order.
pub fn decide(reading: &RosterReading) -> Decision {
    if !reading.roster_supported {
        return Decision::Refuse;
    }
    if reading.missed_polls >= MISSED_POLL_LIMIT {
        return Decision::Pause;
    }
    if reading.participants != 2 {
        return Decision::Pause;
    }
    if reading.was_paused && !reading.reconsented {
        return Decision::Pause;
    }
    Decision::Record
}

fn sentences() -> &'static [(&'static str, &'static str)] {
    use std::sync::OnceLock;
    static SENTENCES: OnceLock<Vec<(&'static str, &'static str)>> = OnceLock::new();
    SENTENCES
        .get_or_init(|| {
            let export: serde_json::Value =
                serde_json::from_str(VENDORED_JSON).expect("the vendored export parses");
            export["sentences"]
                .as_object()
                .expect("the export carries sentences")
                .iter()
                .map(|(k, v)| {
                    let key: &'static str = Box::leak(k.clone().into_boxed_str());
                    let sentence: &'static str = Box::leak(
                        v.as_str()
                            .expect("a sentence is a string")
                            .to_string()
                            .into_boxed_str(),
                    );
                    (key, sentence)
                })
                .collect()
        })
        .as_slice()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_people_on_a_platform_that_lists_them_keeps_recording() {
        assert_eq!(decide(&RosterReading::default()), Decision::Record);
    }

    #[test]
    fn a_third_participant_pauses_capture() {
        let reading = RosterReading {
            participants: 3,
            ..Default::default()
        };
        assert_eq!(decide(&reading), Decision::Pause);
    }

    #[test]
    fn a_platform_with_no_roster_refuses_to_record_at_all() {
        let reading = RosterReading {
            roster_supported: false,
            ..Default::default()
        };
        assert_eq!(decide(&reading), Decision::Refuse);
    }

    #[test]
    fn a_poll_the_agent_could_not_complete_pauses_capture() {
        let reading = RosterReading {
            missed_polls: 1,
            ..Default::default()
        };
        assert_eq!(
            decide(&reading),
            Decision::Pause,
            "not knowing who is in the call is the same as knowing someone else is"
        );
    }

    #[test]
    fn a_pause_does_not_lift_when_the_third_person_leaves() {
        let reading = RosterReading {
            participants: 2,
            was_paused: true,
            reconsented: false,
            ..Default::default()
        };
        assert_eq!(decide(&reading), Decision::Pause);

        let after_reconsent = RosterReading {
            reconsented: true,
            ..reading
        };
        assert_eq!(decide(&after_reconsent), Decision::Record);
    }

    #[test]
    fn recording_implies_every_condition_over_all_readings() {
        for participants in 0..5u32 {
            for missed_polls in 0..3u32 {
                for roster_supported in [true, false] {
                    for was_paused in [true, false] {
                        for reconsented in [true, false] {
                            let reading = RosterReading {
                                participants,
                                roster_supported,
                                missed_polls,
                                was_paused,
                                reconsented,
                            };
                            if decide(&reading) == Decision::Record {
                                assert!(roster_supported);
                                assert_eq!(participants, 2);
                                assert_eq!(missed_polls, 0);
                                assert!(!was_paused || reconsented);
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn every_sentence_and_both_bounds_still_match_the_engine() {
        let export: serde_json::Value = serde_json::from_str(VENDORED_JSON).unwrap();

        assert_eq!(
            export["missed_poll_limit"].as_u64().unwrap(),
            u64::from(MISSED_POLL_LIMIT)
        );
        assert_eq!(
            export["pause_within_ms"].as_u64().unwrap(),
            PAUSE_WITHIN.as_millis() as u64
        );
        for (decision, key) in [
            (Decision::Record, "record"),
            (Decision::Pause, "pause"),
            (Decision::Refuse, "refuse"),
        ] {
            assert_eq!(
                decision.sentence(),
                export["sentences"][key].as_str().unwrap(),
                "{key}: the engine reworded this; update the agent rather than \
                 letting it say something else"
            );
        }
    }
}
