//! The sentences the agent says when it refuses to record.
//!
//! The wording is not the agent's to invent. Darpana decides what a refusal
//! means and therefore how it reads, in `engine/capture/refusals.py`, which
//! exports `capture-refusals.json`; that file is vendored next to this module
//! and the test below asserts these constants still match it. A reworded
//! refusal in the engine fails this test until the agent is updated, rather
//! than leaving two systems telling the coach different things.

/// The output device was audible in the microphone.
pub const ECHO_HEARD_IN_MICROPHONE: &str =
    "Recording stopped: your speakers can be heard in your microphone, so the other \
     person's voice would be recorded onto your track and could not be separated or \
     deleted later. Put on headphones and start again.";

/// The echo test itself could not be performed.
pub const ECHO_TEST_COULD_NOT_RUN: &str =
    "Recording stopped: the echo test could not be run on your output device, so there \
     is no evidence your microphone is free of the other person's voice.";

/// A capture device has already summed both sources.
pub const DEVICE_IS_PRE_MIXED: &str =
    "Recording stopped: the selected device already mixes both sides of the call into \
     one signal, so the two voices could never be separated or deleted again. Choose \
     your microphone and your system audio as separate devices.";

/// The engine's copy of the above, vendored from Darpana.
const VENDORED_JSON: &str = include_str!("capture-refusals.json");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sentence_still_matches_the_engines_wording() {
        let engine: serde_json::Value =
            serde_json::from_str(VENDORED_JSON).expect("the vendored export parses");

        for (key, ours) in [
            ("echo_heard_in_microphone", ECHO_HEARD_IN_MICROPHONE),
            ("echo_test_could_not_run", ECHO_TEST_COULD_NOT_RUN),
            ("device_is_pre_mixed", DEVICE_IS_PRE_MIXED),
        ] {
            let theirs = engine
                .get(key)
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| panic!("the engine no longer exports {key}"));
            assert_eq!(
                ours, theirs,
                "{key}: the engine reworded this refusal; update the constant \
                 rather than letting the agent say something else"
            );
        }
    }
}
