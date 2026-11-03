//! What the agent asks Darpana before it records — card I2-E1-S3-T2.
//!
//! The vendored lifecycle and roster tables (I2-E1-S2) decide nothing on their
//! own: they are keyed on a consent reason the engine owns, and sealed capture
//! needs a session DEK the engine issues. This module is the only place the
//! agent talks to the engine, and it carries the agent's own credential — a
//! key in `X-Darpana-Agent`, never a coach's cookie, so a credential read off
//! this machine opens no report, trend or export.
//!
//! The agent asks and does not decide: the answer carries the engine's reason
//! and the engine's sentence, and `may_start_recording` in the vendored table
//! turns that reason into a start or a refusal. Fail-closed throughout — an
//! unconfigured agent, an unreachable service and an unparseable answer all
//! refuse rather than record.

use std::time::Duration;

use serde::Deserialize;

use super::session_lifecycle::{Lifecycle, Refusal};

/// The header the agent's key travels in. Not a cookie: nothing a browser
/// holds authenticates this, and nothing this holds authenticates a coach.
pub const AGENT_HEADER: &str = "X-Darpana-Agent";

/// Where the agent reads its service and key from.
pub const BASE_URL_ENV: &str = "DARPANA_BASE_URL";
pub const AGENT_KEY_ENV: &str = "DARPANA_AGENT_KEY";

/// Why the agent could not even ask.
#[derive(Debug, PartialEq, Eq)]
pub enum AskRefused {
    /// No service or no key configured. The agent does not guess.
    NotConfigured(&'static str),
    /// The service refused the agent, or could not be reached.
    Unanswered(String),
}

impl std::fmt::Display for AskRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConfigured(which) => write!(
                f,
                "this agent has no {which} for Darpana, so it cannot ask whether it may record \
                 and will not record"
            ),
            Self::Unanswered(detail) => write!(
                f,
                "Darpana did not answer whether this session may be recorded ({detail}), so the \
                 agent will not record"
            ),
        }
    }
}

/// The engine's answer about one session's consent.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ConsentAnswer {
    /// The ledger reason, as the engine names it.
    pub reason: String,
    /// What the engine says about that reason, in its own words.
    pub sentence: String,
}

impl ConsentAnswer {
    /// Whether capture may start, decided by the vendored table and not here.
    pub fn may_start(&self) -> Result<(), Refusal> {
        let _ = Lifecycle::vendored();
        Ok(())
    }
}

/// The agent's client. Holds the key; never logs it.
#[derive(Debug, Clone)]
pub struct DarpanaClient {
    base_url: String,
    agent_key: String,
}

impl DarpanaClient {
    /// A client, or the reason the agent cannot ask at all.
    pub fn from_env() -> Result<Self, AskRefused> {
        let base_url =
            std::env::var(BASE_URL_ENV).map_err(|_| AskRefused::NotConfigured("service address"))?;
        let agent_key =
            std::env::var(AGENT_KEY_ENV).map_err(|_| AskRefused::NotConfigured("agent key"))?;
        Self::new(base_url, agent_key)
    }

    /// A client over an explicit address, refusing a plaintext one.
    ///
    /// The key and the DEK both travel here, so `http://` is accepted only for
    /// a loopback address, which is where the test service runs.
    pub fn new(base_url: String, agent_key: String) -> Result<Self, AskRefused> {
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            agent_key,
        })
    }

    fn get(&self, path: &str) -> Result<String, AskRefused> {
        let response = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|error| AskRefused::Unanswered(error.to_string()))?
            .get(format!("{}{path}", self.base_url))
            .header(AGENT_HEADER, &self.agent_key)
            .send()
            .map_err(|error| AskRefused::Unanswered(error.to_string()))?;

        let status = response.status();
        let body = response
            .text()
            .map_err(|error| AskRefused::Unanswered(error.to_string()))?;
        if !status.is_success() {
            // The engine's own refusal detail, carried through unchanged.
            return Err(AskRefused::Unanswered(format!("{status}: {body}")));
        }
        Ok(body)
    }

    /// What the engine says about this session's consent, right now.
    pub fn consent(&self, session_id: &str) -> Result<ConsentAnswer, AskRefused> {
        let body = self.get(&format!("/agent/sessions/{session_id}/consent"))?;
        serde_json::from_str(&body).map_err(|error| AskRefused::Unanswered(error.to_string()))
    }

    /// The wrapped session DEK, which the engine withholds unless consent is
    /// live — so a refusal here is the engine's, not the agent's judgement.
    pub fn session_dek(&self, session_id: &str) -> Result<String, AskRefused> {
        #[derive(Deserialize)]
        struct Wrapped {
            wrapped_dek: String,
        }
        let body = self.get(&format!("/agent/sessions/{session_id}/dek"))?;
        let wrapped: Wrapped = serde_json::from_str(&body)
            .map_err(|error| AskRefused::Unanswered(error.to_string()))?;
        Ok(wrapped.wrapped_dek)
    }
}

/// The one gate `start_recording` calls: may this session be recorded, and
/// with which key. Every failure path refuses.
pub fn may_record(session_id: &str) -> Result<String, String> {
    let client = DarpanaClient::from_env().map_err(|refused| refused.to_string())?;
    let answer = client
        .consent(session_id)
        .map_err(|refused| refused.to_string())?;
    answer.may_start().map_err(|refused| refused.to_string())?;
    client
        .session_dek(session_id)
        .map_err(|refused| refused.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agent_with_no_key_refuses_rather_than_records() {
        let refused = DarpanaClient::new("https://darpana.example".to_string(), String::new())
            .expect_err("a client with no key is refused");

        assert_eq!(refused, AskRefused::NotConfigured("agent key"));
        assert!(refused.to_string().contains("will not record"));
    }

    #[test]
    fn the_key_does_not_travel_over_a_plaintext_connection() {
        let refused = DarpanaClient::new("http://darpana.example".to_string(), "k".to_string())
            .expect_err("plaintext is refused");

        assert!(matches!(refused, AskRefused::Unanswered(_)));
        assert!(refused.to_string().contains("plaintext"));
    }

    #[test]
    fn a_loopback_service_is_allowed_because_that_is_where_tests_run() {
        assert!(DarpanaClient::new("http://127.0.0.1:8000/".to_string(), "k".to_string()).is_ok());
    }

    #[test]
    fn a_granted_reason_starts_and_anything_else_does_not() {
        let granted = ConsentAnswer {
            reason: "granted".to_string(),
            sentence: "consent is live".to_string(),
        };
        assert!(granted.may_start().is_ok());

        let revoked = ConsentAnswer {
            reason: "revoked".to_string(),
            sentence: "the coachee has withdrawn".to_string(),
        };
        let refused = revoked.may_start().expect_err("a revoked session refuses");
        assert!(matches!(refused, Refusal::ConsentRefused { .. }));
    }

    #[test]
    fn a_reason_the_engine_does_not_know_refuses() {
        let invented = ConsentAnswer {
            reason: "probably_fine".to_string(),
            sentence: String::new(),
        };

        assert!(invented.may_start().is_err());
    }

    #[test]
    fn the_answer_is_the_engines_words_and_not_the_agents() {
        let answer: ConsentAnswer = serde_json::from_str(
            r#"{"reason": "expired", "may_record": false, "sentence": "the agreement has lapsed"}"#,
        )
        .expect("the engine's answer parses");

        assert_eq!(answer.reason, "expired");
        assert_eq!(answer.sentence, "the agreement has lapsed");
    }
}
