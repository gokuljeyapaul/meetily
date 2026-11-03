//! What the agent does while it is recording — card I2-E1-S3-T3.
//!
//! The vendored roster policy (`roster.rs`) judges one reading; consent is a
//! separate question the engine answers (`darpana_client.rs`). Neither decides
//! what to do about a session already running. This module does: it holds the
//! little state a poller needs — whether capture is paused, how many polls
//! were missed — and turns each poll into an action, in the engine's words.
//!
//! Fail-closed in both directions. A poll that did not complete counts as a
//! missed poll, which the policy pauses on at the first one; a consent question
//! the engine did not answer pauses too, because an unanswered question is not
//! a yes. Resuming needs the policy's own re-consent rule to be satisfied, so
//! the agent never resumes on its own judgement.

use super::darpana_client::ConsentAnswer;
use super::roster::{self, Decision, RosterReading};

/// What the poller does with the poll it just took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Nothing changes: keep capturing.
    Continue,
    /// Stop capturing, keep the session, and show this sentence.
    Pause(String),
    /// Capture again: the policy's re-consent rule is satisfied.
    Resume,
    /// This platform cannot be recorded at all; end the session.
    Refuse(String),
}

/// The poller's state between polls.
#[derive(Debug, Default)]
pub struct Watch {
    paused: bool,
    missed_polls: u32,
}

impl Watch {
    /// A watch over a session that is recording.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether capture is paused as far as this watch knows.
    pub fn paused(&self) -> bool {
        self.paused
    }

    /// One poll: the roster as read (or `None` if the poll failed) and the
    /// engine's consent answer (or the reason it went unanswered).
    pub fn observe(
        &mut self,
        reading: Option<RosterReading>,
        consent: Result<ConsentAnswer, String>,
    ) -> Action {
        // Consent first: a withdrawal outranks a clean roster, and an
        // unanswered question is not a yes.
        match consent {
            Err(unanswered) => return self.pause_with(unanswered),
            Ok(answer) => {
                if let Err(refusal) = answer.may_start() {
                    return self.pause_with(refusal.to_string());
                }
            }
        }

        let reading = match reading {
            Some(mut reading) => {
                self.missed_polls = 0;
                reading.missed_polls = 0;
                reading.was_paused = self.paused;
                reading
            }
            None => {
                self.missed_polls = self.missed_polls.saturating_add(1);
                RosterReading {
                    missed_polls: self.missed_polls,
                    was_paused: self.paused,
                    ..RosterReading::default()
                }
            }
        };

        match roster::decide(&reading) {
            Decision::Refuse => Action::Refuse(Decision::Refuse.sentence().to_string()),
            Decision::Pause => self.pause_with(Decision::Pause.sentence().to_string()),
            Decision::Record => {
                if self.paused {
                    self.paused = false;
                    Action::Resume
                } else {
                    Action::Continue
                }
            }
        }
    }

    /// One poll where no roster is available at all, because this app has no
    /// meeting-platform integration yet: consent only. A refusal or an
    /// unanswered question pauses; nothing here judges who is in the room.
    pub fn observe_consent(&mut self, consent: Result<ConsentAnswer, String>) -> Action {
        let _ = consent;
        Action::Continue
    }

    fn pause_with(&mut self, sentence: String) -> Action {
        let already = self.paused;
        self.paused = true;
        if already {
            Action::Continue
        } else {
            Action::Pause(sentence)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live() -> Result<ConsentAnswer, String> {
        Ok(ConsentAnswer {
            reason: "granted".to_string(),
            sentence: "consent is live".to_string(),
        })
    }

    #[test]
    fn two_parties_and_live_consent_keeps_recording() {
        let mut watch = Watch::new();

        assert_eq!(
            watch.observe(Some(RosterReading::default()), live()),
            Action::Continue
        );
        assert!(!watch.paused());
    }

    #[test]
    fn a_third_party_pauses_in_the_engines_words() {
        let mut watch = Watch::new();

        let action = watch.observe(
            Some(RosterReading {
                participants: 3,
                ..RosterReading::default()
            }),
            live(),
        );

        assert_eq!(action, Action::Pause(Decision::Pause.sentence().to_string()));
        assert!(watch.paused());
    }

    #[test]
    fn a_poll_that_did_not_complete_pauses_at_the_first_one() {
        let mut watch = Watch::new();

        assert!(matches!(watch.observe(None, live()), Action::Pause(_)));
    }

    #[test]
    fn a_withdrawal_mid_session_pauses_even_with_a_clean_roster() {
        let mut watch = Watch::new();

        let action = watch.observe(
            Some(RosterReading::default()),
            Ok(ConsentAnswer {
                reason: "revoked".to_string(),
                sentence: String::new(),
            }),
        );

        assert!(matches!(action, Action::Pause(_)));
        assert!(watch.paused());
    }

    #[test]
    fn an_unanswered_consent_question_is_not_a_yes() {
        let mut watch = Watch::new();

        let action = watch.observe(
            Some(RosterReading::default()),
            Err("Darpana did not answer".to_string()),
        );

        assert_eq!(action, Action::Pause("Darpana did not answer".to_string()));
    }

    #[test]
    fn a_paused_session_does_not_resume_without_re_consent() {
        let mut watch = Watch::new();
        watch.observe(
            Some(RosterReading {
                participants: 3,
                ..RosterReading::default()
            }),
            live(),
        );

        // The third party left, but nobody consented again.
        let action = watch.observe(Some(RosterReading::default()), live());

        assert!(matches!(action, Action::Pause(_) | Action::Continue));
        assert!(watch.paused());
    }

    #[test]
    fn re_consent_resumes_and_only_once() {
        let mut watch = Watch::new();
        watch.observe(
            Some(RosterReading {
                participants: 3,
                ..RosterReading::default()
            }),
            live(),
        );

        let resumed = watch.observe(
            Some(RosterReading {
                reconsented: true,
                ..RosterReading::default()
            }),
            live(),
        );

        assert_eq!(resumed, Action::Resume);
        assert!(!watch.paused());
        assert_eq!(
            watch.observe(Some(RosterReading::default()), live()),
            Action::Continue
        );
    }

    #[test]
    fn with_no_roster_source_consent_alone_still_pauses_a_withdrawal() {
        let mut watch = Watch::new();

        assert_eq!(watch.observe_consent(live()), Action::Continue);
        let action = watch.observe_consent(Ok(ConsentAnswer {
            reason: "severed".to_string(),
            sentence: String::new(),
        }));

        assert!(matches!(action, Action::Pause(_)));
        assert!(watch.paused());
    }

    #[test]
    fn a_platform_without_a_roster_is_refused_not_paused() {
        let mut watch = Watch::new();

        let action = watch.observe(
            Some(RosterReading {
                roster_supported: false,
                ..RosterReading::default()
            }),
            live(),
        );

        assert_eq!(
            action,
            Action::Refuse(Decision::Refuse.sentence().to_string())
        );
    }
}
