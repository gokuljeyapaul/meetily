//! The echo gate: refuse to record when the coachee comes back in through the
//! coach's microphone.
//!
//! Writing two unmixed tracks (`two_track.rs`) separates the sources in code.
//! Physics can put them back together: if the coach listens on speakers, the
//! coachee's voice leaves the output device, crosses the room and arrives at
//! the coach's microphone, so coachee speech lands on the coach track with no
//! code involved. Severance then cannot delete it, and a quote attributed to
//! the coach may be the coachee's.
//!
//! So at session start the agent plays a short known tone through the output
//! device and measures that tone's level in the microphone. Above the
//! threshold it refuses to record and says why. The test is re-run whenever
//! the output device changes mid-session, and an output device that cannot be
//! driven is a refusal too, never a pass: a gate that cannot measure has not
//! measured anything.
//!
//! The threshold is derived, not chosen by ear. The microphone's own noise
//! floor is measured immediately before the probe, and the probe is a refusal
//! only when the tone stands `ECHO_MARGIN_DB` above that floor at the probe
//! frequency. A quiet room therefore gets a stricter absolute threshold than a
//! noisy one, and both numbers are recorded with the verdict.

use std::f32::consts::TAU;

use anyhow::{anyhow, Result};

use super::refusals;

/// The probe frequency. Speech energy is dense around it, so a room that
/// passes here is a room that will not pass back the coachee's voice; and it
/// is narrow enough to be measured against the floor at that frequency alone
/// rather than against broadband noise.
pub const PROBE_TONE_HZ: f32 = 1_000.0;

/// How long the tone plays. Long enough for the measurement to average over
/// several cycles, short enough that the coach hears a blip rather than a beep.
pub const PROBE_DURATION_MS: u64 = 250;

/// The probe's own amplitude, about -12 dBFS: loud enough to be measurable
/// over a normal floor, well short of clipping the output device.
pub const PROBE_AMPLITUDE: f32 = 0.25;

/// How far above the measured floor the probe tone must stand before the
/// recording is refused.
///
/// 6 dB is a factor of two in amplitude. Below that, the measurement cannot
/// separate a real echo path from the floor's own variation between the two
/// windows; above it, the tone is audibly present in the microphone.
pub const ECHO_MARGIN_DB: f32 = 6.0;

/// What the microphone heard: the floor on its own, then the same microphone
/// while the probe played.
#[derive(Debug, Clone)]
pub struct ProbeCapture {
    /// Microphone samples captured with the output device silent.
    pub floor: Vec<f32>,
    /// Microphone samples captured while the probe tone played.
    pub during_probe: Vec<f32>,
}

/// Plays the probe through the output device and hands back what the
/// microphone heard. Implemented against real devices in the app, and against
/// synthetic rooms in the tests.
pub trait ProbePlayer {
    /// The output device being tested, so a change of device can re-run the test.
    fn output_device_name(&self) -> String;

    /// Play `probe` on the output device and capture the microphone through it.
    ///
    /// An error means the device could not be driven, which is a refusal.
    fn play_probe(&mut self, probe: &[f32], sample_rate: u32) -> Result<ProbeCapture>;
}

/// The numbers behind a verdict, kept so the refusal can quote them and the
/// threshold is never a matter of opinion.
#[derive(Debug, Clone)]
pub struct EchoMeasurement {
    pub output_device: String,
    /// The microphone's level at the probe frequency with the output silent.
    pub floor_dbfs: f32,
    /// The microphone's level at the probe frequency while the probe played.
    pub probe_dbfs: f32,
    /// How far the probe stands above the floor.
    pub excess_db: f32,
}

impl EchoMeasurement {
    /// True when the output device is audible in the microphone.
    pub fn is_echoing(&self) -> bool {
    unimplemented!("I2-E1-S1-T3: the echo gate is not built yet")
}
}

/// The gate's answer.
#[derive(Debug, Clone)]
pub enum EchoVerdict {
    /// Nothing of the output device reached the microphone: headphones, or a
    /// room quiet enough to be indistinguishable from them.
    Clear(EchoMeasurement),
    /// The output device was heard in the microphone. Recording is refused.
    OutputHeardInMicrophone(EchoMeasurement),
}

impl EchoVerdict {
    pub fn measurement(&self) -> &EchoMeasurement {
        match self {
            EchoVerdict::Clear(m) | EchoVerdict::OutputHeardInMicrophone(m) => m,
        }
    }

    pub fn allows_recording(&self) -> bool {
        matches!(self, EchoVerdict::Clear(_))
    }

    /// The sentence shown to the coach when the gate refuses. The wording
    /// belongs to the engine (see `refusals`), not to this module.
    pub fn refusal_sentence(&self) -> Option<String> {
    unimplemented!("I2-E1-S1-T3: the echo gate is not built yet")
}
}

/// What the recording lifecycle must do after a mid-session re-test.
#[derive(Debug, Clone)]
pub enum GateAction {
    /// The output device is unchanged, or changed and still clear.
    Continue,
    /// The output device changed and now echoes: pause, with this sentence.
    Pause(String),
}

/// The probe tone itself: `PROBE_DURATION_MS` of a sine at `PROBE_TONE_HZ`.
pub fn probe_signal(sample_rate: u32) -> Vec<f32> {
    unimplemented!("I2-E1-S1-T3: the echo gate is not built yet")
}

/// How many samples each measurement block covers. The level is averaged over
/// blocks rather than measured once across the window: one Goertzel bin of
/// room noise is a two-degree-of-freedom estimate whose single realisation
/// swings tens of dB, which would make a quiet room read as an echo about as
/// often as it read as silence. Averaging ~10 blocks brings that spread down
/// to a couple of dB, which is what makes a 6 dB margin mean anything.
const MEASUREMENT_BLOCK: usize = 1_024;

/// The level of one frequency in `samples`, in dBFS.
///
/// A single-bin Goertzel rather than an FFT: one frequency is all that is
/// being asked about, and measuring the probe's own frequency is what keeps a
/// noisy room from reading as an echo.
pub fn tone_level_dbfs(samples: &[f32], sample_rate: u32, frequency: f32) -> f32 {
    unimplemented!("I2-E1-S1-T3: the echo gate is not built yet")
}

fn block_level_dbfs(samples: &[f32], sample_rate: u32, frequency: f32) -> f32 {
    unimplemented!("I2-E1-S1-T3: the echo gate is not built yet")
}

/// Run the echo test once against whatever the player is driving.
pub fn run_echo_test<P: ProbePlayer + ?Sized>(
    player: &mut P,
    sample_rate: u32,
) -> Result<EchoVerdict> {
    unimplemented!("I2-E1-S1-T3: the echo gate is not built yet")
}

/// The gate across a session: the verdict that let recording start, and the
/// output device it was measured on.
#[derive(Debug)]
pub struct EchoGate {
    sample_rate: u32,
    tested_output_device: String,
    last_measurement: EchoMeasurement,
}

impl EchoGate {
    /// Run the test before recording starts. `Err` means do not record.
    pub fn open<P: ProbePlayer + ?Sized>(player: &mut P, sample_rate: u32) -> Result<Self> {
    unimplemented!("I2-E1-S1-T3: the echo gate is not built yet")
}

    pub fn tested_output_device(&self) -> &str {
        &self.tested_output_device
    }

    pub fn last_measurement(&self) -> &EchoMeasurement {
        &self.last_measurement
    }

    /// Called when the output device may have changed mid-session. The test is
    /// re-run for a new device, and an echo pauses the recording.
    pub fn output_device_changed<P: ProbePlayer + ?Sized>(
        &mut self,
        player: &mut P,
    ) -> Result<GateAction> {
    unimplemented!("I2-E1-S1-T3: the echo gate is not built yet")
}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic room: the probe comes back into the microphone attenuated by
    /// `echo_gain`, on top of a broadband floor.
    struct Room {
        device: String,
        echo_gain: f32,
        floor_amplitude: f32,
        drivable: bool,
    }

    impl Room {
        fn speakers() -> Self {
            Self {
                device: "MacBook Pro Speakers".to_string(),
                // -26 dB of the probe returns: a laptop at conversational volume.
                echo_gain: 0.05,
                floor_amplitude: 0.001,
                drivable: true,
            }
        }

        fn headphones() -> Self {
            Self {
                device: "Sony WH-1000XM4".to_string(),
                echo_gain: 0.0,
                floor_amplitude: 0.001,
                drivable: true,
            }
        }

        fn noise(&self, count: usize, seed: u64) -> Vec<f32> {
            let mut state = seed | 1;
            (0..count)
                .map(|_| {
                    state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                    let unit = ((state >> 33) as f32 / (1u64 << 31) as f32) - 1.0;
                    unit * self.floor_amplitude
                })
                .collect()
        }
    }

    impl ProbePlayer for Room {
        fn output_device_name(&self) -> String {
            self.device.clone()
        }

        fn play_probe(&mut self, probe: &[f32], _sample_rate: u32) -> Result<ProbeCapture> {
            if !self.drivable {
                return Err(anyhow!("the output device refused to open a stream"));
            }
            let floor = self.noise(probe.len(), 7);
            let during_probe = self
                .noise(probe.len(), 11)
                .iter()
                .zip(probe)
                .map(|(noise, tone)| noise + tone * self.echo_gain)
                .collect();
            Ok(ProbeCapture {
                floor,
                during_probe,
            })
        }
    }

    /// DoD: speaker output detected in the mic refuses the recording with the
    /// engine's sentence.
    #[test]
    fn speakers_refuse_the_recording_with_the_engines_sentence() {
        let mut room = Room::speakers();

        let verdict = run_echo_test(&mut room, 48_000).expect("the probe played");
        let measurement = verdict.measurement();
        assert!(
            !verdict.allows_recording(),
            "speakers audible in the microphone must not allow recording; \
             measured {:.1} dB above a {:.1} dBFS floor",
            measurement.excess_db,
            measurement.floor_dbfs
        );
        assert!(
            measurement.excess_db >= ECHO_MARGIN_DB,
            "the refusal must rest on the measured margin, not on the device name"
        );

        let sentence = verdict.refusal_sentence().expect("a refusal says why");
        assert_eq!(
            sentence.split(" (").next().unwrap(),
            refusals::ECHO_HEARD_IN_MICROPHONE,
            "the wording is the engine's, not this module's"
        );
        assert!(
            sentence.contains("MacBook Pro Speakers"),
            "the sentence names the device that failed: {sentence}"
        );

        // And the gate itself refuses to open.
        let opened = EchoGate::open(&mut room, 48_000);
        assert!(opened.is_err(), "the gate must not open on speakers");
    }

    /// DoD: headphones (no detected output) start the recording.
    #[test]
    fn headphones_start_the_recording() {
        let mut room = Room::headphones();

        let gate = EchoGate::open(&mut room, 48_000).expect("headphones pass the gate");

        assert_eq!(gate.tested_output_device(), "Sony WH-1000XM4");
        assert!(
            gate.last_measurement().excess_db < ECHO_MARGIN_DB,
            "with nothing coming back, the probe window must sit at the floor; \
             measured {:.1} dB",
            gate.last_measurement().excess_db
        );
        assert!(
            run_echo_test(&mut room, 48_000)
                .expect("the probe played")
                .allows_recording()
        );
    }

    /// DoD: switching from headphones to speakers mid-session re-runs the test
    /// and pauses.
    #[test]
    fn switching_to_speakers_mid_session_re_runs_the_test_and_pauses() {
        let mut headphones = Room::headphones();
        let mut gate = EchoGate::open(&mut headphones, 48_000).expect("headphones pass");

        // Nothing changed: no re-test, and no interruption.
        assert!(matches!(
            gate.output_device_changed(&mut headphones).unwrap(),
            GateAction::Continue
        ));

        // The coach unplugs the headphones mid-session.
        let mut speakers = Room::speakers();
        let action = gate
            .output_device_changed(&mut speakers)
            .expect("the re-test ran");

        match action {
            GateAction::Pause(sentence) => {
                assert_eq!(
                    sentence.split(" (").next().unwrap(),
                    refusals::ECHO_HEARD_IN_MICROPHONE
                );
                assert!(sentence.contains("MacBook Pro Speakers"));
            }
            GateAction::Continue => {
                panic!("a mid-session switch to speakers must pause the recording")
            }
        }
        assert_eq!(
            gate.tested_output_device(),
            "MacBook Pro Speakers",
            "the re-test is recorded against the new device"
        );
    }

    /// DoD: an output device that cannot be driven refuses rather than passing
    /// by default.
    #[test]
    fn an_undrivable_output_device_refuses() {
        let mut dead = Room {
            drivable: false,
            ..Room::headphones()
        };

        let error = run_echo_test(&mut dead, 48_000)
            .expect_err("a device that cannot be driven is not a pass");
        assert!(
            error.to_string().starts_with(refusals::ECHO_TEST_COULD_NOT_RUN),
            "the refusal wording is the engine's: {error}"
        );
        assert!(EchoGate::open(&mut dead, 48_000).is_err());

        // A device that opens but returns nothing is the same refusal: silence
        // read as "no echo" is the failure mode this card exists to prevent.
        struct SilentDevice;
        impl ProbePlayer for SilentDevice {
            fn output_device_name(&self) -> String {
                "USB Audio CODEC".to_string()
            }
            fn play_probe(&mut self, _probe: &[f32], _sample_rate: u32) -> Result<ProbeCapture> {
                Ok(ProbeCapture {
                    floor: Vec::new(),
                    during_probe: Vec::new(),
                })
            }
        }
        let error = run_echo_test(&mut SilentDevice, 48_000)
            .expect_err("no samples is not a measurement");
        assert!(error
            .to_string()
            .starts_with(refusals::ECHO_TEST_COULD_NOT_RUN));

        // And a device that disappears mid-session pauses rather than coasting
        // on the verdict measured for the old one.
        let mut gate = EchoGate::open(&mut Room::headphones(), 48_000).expect("headphones pass");
        let mut gone = Room {
            device: "Unplugged Dock".to_string(),
            drivable: false,
            ..Room::headphones()
        };
        assert!(matches!(
            gate.output_device_changed(&mut gone).unwrap(),
            GateAction::Pause(_)
        ));
    }

    #[test]
    fn the_probe_is_a_known_tone_and_the_meter_measures_it() {
        let probe = probe_signal(48_000);
        assert_eq!(probe.len(), 12_000, "250 ms at 48 kHz");

        let level = tone_level_dbfs(&probe, 48_000, PROBE_TONE_HZ);
        let expected = 20.0 * PROBE_AMPLITUDE.log10();
        assert!(
            (level - expected).abs() < 1.0,
            "the meter must read the probe's own amplitude: {level:.1} dBFS vs {expected:.1} dBFS"
        );

        // A loud room at another frequency is not an echo of this probe.
        let off_tone: Vec<f32> = (0..12_000)
            .map(|n| 0.5 * (TAU * 220.0 * n as f32 / 48_000.0).sin())
            .collect();
        assert!(
            tone_level_dbfs(&off_tone, 48_000, PROBE_TONE_HZ) < -30.0,
            "the meter must be narrow enough to ignore unrelated noise"
        );
    }
}
