//! Two separate tracks, written straight from the devices that produced them.
//!
//! Upstream mixes the microphone and the system-audio stream into one file and
//! labels the result "microphone". For Darpana that single file is the thing
//! that makes severance unenforceable: once the coachee's voice is summed into
//! the coach's track, no downstream code can take it out again, and deleting
//! coachee content means deleting the coach's audio with it.
//!
//! So the recording path here never mixes. The microphone device writes
//! `coach.wav`, the system-audio capture writes `coachee.wav`, each with its own
//! sample count, and the device identifier that produced it is recorded beside
//! it. Mixing still happens in `pipeline.rs`, but only to feed the live
//! transcript; nothing mixed reaches disk.
//!
//! Separation here is structural, never statistical: this module has no
//! diarisation or source-separation step to fall back on, because a mixed file
//! is not something to be repaired later.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

#[allow(unused_imports)]
use super::recording_state::{AudioChunk, DeviceType};

/// The alignment tolerance, in samples at the capture rate: the two devices run
/// on independent clocks, so their sample counts drift apart over a long call.
/// One 10 ms frame is the budget within which the two files' durations must
/// agree after `finish` pads the shorter one.
pub const FRAME_SAMPLES_AT_48K: u64 = 480;

/// Device names that denote an endpoint which has already summed the two
/// sources. Capturing from one of these would hand us a mixed track under a
/// per-source name, which is worse than failing.
const PRE_MIXED_DEVICE_MARKERS: &[&str] = &[
    "stereo mix",
    "aggregate",
    "multi-output",
    "what you hear",
    "mixed",
    "mix (",
];

/// Refuse a device whose own name says it is already a mix of both sources.
pub fn check_device_is_single_source(_role: &str, _device_name: &str) -> Result<()> {
    unimplemented!("two-track capture is not implemented yet")
}

/// What one finished track is, for the metadata the engine reads.
#[derive(Debug, Clone)]
pub struct TrackResult {
    pub path: PathBuf,
    pub device_name: String,
    pub samples: u64,
}

/// Both finished tracks.
#[derive(Debug, Clone)]
pub struct TwoTrackResult {
    pub coach: TrackResult,
    pub coachee: TrackResult,
    pub sample_rate: u32,
}

impl TwoTrackResult {
    pub fn duration_seconds(_track: &TrackResult, _sample_rate: u32) -> f64 {
        unimplemented!("two-track capture is not implemented yet")
    }
}

/// Routes each chunk to the file belonging to the device that produced it.
#[derive(Debug)]
pub struct TwoTrackRecorder;

impl TwoTrackRecorder {
    pub fn new(
        _meeting_folder: &Path,
        _sample_rate: u32,
        _mic_device: &str,
        _system_device: &str,
    ) -> Result<Self> {
        unimplemented!("two-track capture is not implemented yet")
    }

    pub fn write_chunk(&mut self, _chunk: &AudioChunk) -> Result<()> {
        unimplemented!("two-track capture is not implemented yet")
    }

    pub fn coach_samples(&self) -> u64 {
        unimplemented!("two-track capture is not implemented yet")
    }

    pub fn coachee_samples(&self) -> u64 {
        unimplemented!("two-track capture is not implemented yet")
    }

    pub fn finish(self) -> Result<TwoTrackResult> {
        unimplemented!("two-track capture is not implemented yet")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::StdRng;
    use rand::{Rng, SeedableRng};

    fn chunk(device_type: DeviceType, data: Vec<f32>) -> AudioChunk {
        AudioChunk {
            data,
            sample_rate: 48000,
            timestamp: 0.0,
            chunk_id: 0,
            device_type,
        }
    }

    /// Reads a mono 16-bit WAV back as samples, so a test can ask what is
    /// actually on a track rather than what the writer believes it wrote.
    fn read_wav(path: &Path) -> Vec<f32> {
        let bytes = std::fs::read(path).expect("track file exists");
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        let declared = u32::from_le_bytes(bytes[40..44].try_into().unwrap()) as usize;
        assert_eq!(
            declared,
            bytes.len() - 44,
            "declared data length must match the bytes on disk"
        );
        bytes[44..]
            .chunks_exact(2)
            .map(|pair| i16::from_le_bytes([pair[0], pair[1]]) as f32 / i16::MAX as f32)
            .collect()
    }

    fn peak(samples: &[f32]) -> f32 {
        samples.iter().fold(0.0f32, |acc, s| acc.max(s.abs()))
    }

    #[test]
    fn a_capture_writes_two_files_and_the_coach_track_holds_no_system_audio() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder =
            TwoTrackRecorder::new(dir.path(), 48000, "Built-in Microphone", "BlackHole 2ch")
                .unwrap();

        // Ten seconds of a loud tone on the system track only, and speech-like
        // signal on the microphone: if any mixing survived, the coach track
        // would carry the tone.
        let seconds = 10;
        for second in 0..seconds {
            let tone: Vec<f32> = (0..48000)
                .map(|i| {
                    let t = (second * 48000 + i) as f32 / 48000.0;
                    (t * 440.0 * std::f32::consts::TAU).sin() * 0.9
                })
                .collect();
            recorder
                .write_chunk(&chunk(DeviceType::System, tone))
                .unwrap();
            recorder
                .write_chunk(&chunk(DeviceType::Microphone, vec![0.2; 48000]))
                .unwrap();
        }

        let result = recorder.finish().unwrap();
        assert!(result.coach.path.exists(), "coach.wav written");
        assert!(result.coachee.path.exists(), "coachee.wav written");
        assert_eq!(result.coach.device_name, "Built-in Microphone");
        assert_eq!(result.coachee.device_name, "BlackHole 2ch");

        let coach = read_wav(&result.coach.path);
        let coachee = read_wav(&result.coachee.path);
        assert_eq!(coach.len(), 48000 * seconds as usize);
        assert_eq!(coachee.len(), 48000 * seconds as usize);

        // The tone peaks at 0.9; the coach track must stay at its own constant
        // 0.2 with nothing summed on top, measured against quantisation error.
        let floor = 2.0 / i16::MAX as f32;
        assert!(
            coach.iter().all(|s| (s - 0.2).abs() <= floor),
            "coach track carries system audio: peak {}",
            peak(&coach)
        );
        assert!(peak(&coachee) > 0.8, "coachee track lost its own signal");
    }

    #[test]
    fn a_pre_mixed_device_makes_the_capture_refuse() {
        let dir = tempfile::tempdir().unwrap();

        for (mic, system) in [
            ("Stereo Mix (Realtek)", "BlackHole 2ch"),
            ("Built-in Microphone", "Aggregate Device"),
            ("Built-in Microphone", "Multi-Output Device"),
        ] {
            let refusal = TwoTrackRecorder::new(dir.path(), 48000, mic, system)
                .expect_err("a pre-mixed endpoint must be refused, not recorded");
            let message = refusal.to_string();
            assert!(
                message.contains("refusing to record")
                    && message.contains("could never be deleted again"),
                "refusal must say why in the engine's words, got: {message}"
            );
        }

        assert!(
            TwoTrackRecorder::new(dir.path(), 48000, "Built-in Microphone", "BlackHole 2ch")
                .is_ok(),
            "two single-source devices must be accepted"
        );
    }

    #[test]
    fn deliberate_clock_drift_leaves_the_two_durations_within_one_frame() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder =
            TwoTrackRecorder::new(dir.path(), 48000, "Built-in Microphone", "BlackHole 2ch")
                .unwrap();

        // The system device runs 1200 samples (25 ms) fast over the capture —
        // far more drift than the one-frame tolerance.
        for _ in 0..10 {
            recorder
                .write_chunk(&chunk(DeviceType::Microphone, vec![0.1; 4800]))
                .unwrap();
            recorder
                .write_chunk(&chunk(DeviceType::System, vec![-0.1; 4920]))
                .unwrap();
        }
        assert_ne!(
            recorder.coach_samples(),
            recorder.coachee_samples(),
            "the test must start from real drift"
        );

        let result = recorder.finish().unwrap();
        let coach = TwoTrackResult::duration_seconds(&result.coach, result.sample_rate);
        let coachee = TwoTrackResult::duration_seconds(&result.coachee, result.sample_rate);
        let frame = FRAME_SAMPLES_AT_48K as f64 / 48000.0;
        assert!(
            (coach - coachee).abs() <= frame,
            "durations differ by {:.4}s, more than one {:.4}s frame",
            (coach - coachee).abs(),
            frame
        );

        // Padding is silence, not the other device's audio.
        let padded = read_wav(&result.coach.path);
        assert!(padded[padded.len() - 10..].iter().all(|s| *s == 0.0));
    }

    #[test]
    fn any_sequence_of_device_switches_leaves_one_source_per_file() {
        // The microphone writes positive samples and the system device negative
        // ones, so a sample on the wrong track is visible by its sign whatever
        // order the devices arrive in.
        for seed in 0..64u64 {
            let dir = tempfile::tempdir().unwrap();
            let mut rng = StdRng::seed_from_u64(seed);
            let mut recorder =
                TwoTrackRecorder::new(dir.path(), 48000, "Built-in Microphone", "BlackHole 2ch")
                    .unwrap();

            let mut mic_samples = 0u64;
            let mut system_samples = 0u64;
            for _ in 0..rng.gen_range(1..40) {
                let len = rng.gen_range(1..500);
                if rng.gen_bool(0.5) {
                    recorder
                        .write_chunk(&chunk(DeviceType::Microphone, vec![0.5; len]))
                        .unwrap();
                    mic_samples += len as u64;
                } else {
                    recorder
                        .write_chunk(&chunk(DeviceType::System, vec![-0.5; len]))
                        .unwrap();
                    system_samples += len as u64;
                }
            }

            let result = recorder.finish().unwrap();
            let coach = read_wav(&result.coach.path);
            let coachee = read_wav(&result.coachee.path);

            assert!(
                coach[..mic_samples as usize].iter().all(|s| *s > 0.0),
                "seed {seed}: system audio reached the coach track"
            );
            assert!(
                coachee[..system_samples as usize].iter().all(|s| *s < 0.0),
                "seed {seed}: microphone audio reached the coachee track"
            );
            // Anything beyond a track's own samples is the drift padding.
            assert!(coach[mic_samples as usize..].iter().all(|s| *s == 0.0));
            assert!(coachee[system_samples as usize..].iter().all(|s| *s == 0.0));
        }
    }
}
