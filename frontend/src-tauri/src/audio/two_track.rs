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

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use log::info;

use super::recording_state::{AudioChunk, DeviceType};
use super::sealed_capture::{SealedTrackWriter, SessionDek, SEALED_SUFFIX};

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
pub fn check_device_is_single_source(role: &str, device_name: &str) -> Result<()> {
    let lowered = device_name.to_lowercase();
    if let Some(marker) = PRE_MIXED_DEVICE_MARKERS
        .iter()
        .find(|marker| lowered.contains(*marker))
    {
        return Err(anyhow!(
            "refusing to record: the {role} device \"{device_name}\" is a pre-mixed endpoint \
             (matched \"{marker}\"), so the coach and coachee would share one track and \
             coachee audio could never be deleted again"
        ));
    }
    Ok(())
}

/// Where one track's bytes go: a plain WAV file, or frames sealed with the
/// session key as they are written.
///
/// The sealed variant is not a WAV file at all: a WAV declares its length in a
/// header that has to be rewritten at the end, and a sealed track cannot be
/// seeked back into without decrypting what is already there. The engine reads
/// the frames and the sample rate from the recording's metadata instead.
enum TrackSink {
    Plain(BufWriter<File>),
    Sealed {
        writer: SealedTrackWriter,
        dek: Arc<SessionDek>,
    },
}

/// Deliberately hand-written: a derived `Debug` would print the session key
/// the moment anything logged a recorder.
impl std::fmt::Debug for TrackSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Plain(_) => f.write_str("plain"),
            Self::Sealed { .. } => f.write_str("sealed"),
        }
    }
}

/// One track: one device, one file, its own sample clock.
#[derive(Debug)]
struct TrackWriter {
    path: PathBuf,
    sink: TrackSink,
    device_name: String,
    samples_written: u64,
}

impl TrackWriter {
    fn create(
        path: PathBuf,
        device_name: String,
        sample_rate: u32,
        dek: Option<Arc<SessionDek>>,
    ) -> Result<Self> {
        let (path, sink) = match dek {
            Some(dek) => {
                let sealed_path = sealed_path(&path);
                let writer = SealedTrackWriter::create(&sealed_path)?;
                (sealed_path, TrackSink::Sealed { writer, dek })
            }
            None => {
                let mut file = BufWriter::new(File::create(&path)?);
                write_wav_header(&mut file, sample_rate, 0)?;
                (path, TrackSink::Plain(file))
            }
        };
        Ok(Self {
            path,
            sink,
            device_name,
            samples_written: 0,
        })
    }

    fn write(&mut self, samples: &[f32]) -> Result<()> {
        let mut pcm = Vec::with_capacity(samples.len() * 2);
        for sample in samples {
            let clamped = sample.clamp(-1.0, 1.0);
            pcm.extend_from_slice(&((clamped * i16::MAX as f32) as i16).to_le_bytes());
        }
        self.write_pcm(&pcm)?;
        self.samples_written += samples.len() as u64;
        Ok(())
    }

    /// One chunk of samples becomes one frame: a sealed track is written and
    /// flushed frame by frame, so whatever was captured is either sealed on
    /// disk or was never captured at all.
    fn write_pcm(&mut self, pcm: &[u8]) -> Result<()> {
        match &mut self.sink {
            TrackSink::Plain(file) => file.write_all(pcm)?,
            TrackSink::Sealed { writer, dek } => writer.write(dek, pcm)?,
        }
        Ok(())
    }

    fn pad_silence(&mut self, samples: u64) -> Result<()> {
        let count = usize::try_from(samples).map_err(|_| anyhow!("that is not a drift"))?;
        self.write_pcm(&vec![0u8; count * 2])?;
        self.samples_written += samples;
        Ok(())
    }

    fn finalize(self, sample_rate: u32) -> Result<TrackResult> {
        let Self {
            path,
            sink,
            device_name,
            samples_written,
        } = self;
        match sink {
            TrackSink::Plain(file) => {
                let mut file = file.into_inner()?;
                file.seek(SeekFrom::Start(0))?;
                write_wav_header(&mut file, sample_rate, samples_written)?;
                file.flush()?;
            }
            // Every frame was already flushed as it was sealed, and there is
            // no header to go back and correct.
            TrackSink::Sealed { .. } => {}
        }
        Ok(TrackResult {
            path,
            device_name,
            samples: samples_written,
        })
    }
}

/// `coach.wav` sealed is `coach.wav.enc`: the track it came from stays legible
/// in the name, and the sealed reader refuses anything not ending `.enc`.
fn sealed_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{SEALED_SUFFIX}"));
    path.with_file_name(name)
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
    pub fn duration_seconds(track: &TrackResult, sample_rate: u32) -> f64 {
        track.samples as f64 / sample_rate as f64
    }
}

/// Routes each chunk to the file belonging to the device that produced it.
#[derive(Debug)]
pub struct TwoTrackRecorder {
    coach: TrackWriter,
    coachee: TrackWriter,
    sample_rate: u32,
}

impl TwoTrackRecorder {
    /// `mic_device` and `system_device` are the identifiers the capture layer
    /// resolved; both are recorded with their track and both are refused if
    /// they name a pre-mixed endpoint.
    pub fn new(
        meeting_folder: &Path,
        sample_rate: u32,
        mic_device: &str,
        system_device: &str,
    ) -> Result<Self> {
        Self::with_key(meeting_folder, sample_rate, mic_device, system_device, None)
    }

    /// The same two tracks, sealed as they are written when the engine has
    /// issued this session's key (I2-E1-S3-T4). Without a key the tracks are
    /// plain WAV, which only the fixture paths and upstream's tests use.
    pub fn with_key(
        meeting_folder: &Path,
        sample_rate: u32,
        mic_device: &str,
        system_device: &str,
        key: Option<Arc<SessionDek>>,
    ) -> Result<Self> {
        check_device_is_single_source("microphone", mic_device)?;
        check_device_is_single_source("system-audio", system_device)?;

        let coach = TrackWriter::create(
            meeting_folder.join("coach.wav"),
            mic_device.to_string(),
            sample_rate,
            key.clone(),
        )?;
        let coachee = TrackWriter::create(
            meeting_folder.join("coachee.wav"),
            system_device.to_string(),
            sample_rate,
            key,
        )?;

        info!(
            "Recording two unmixed tracks: coach.wav from \"{}\", coachee.wav from \"{}\"",
            mic_device, system_device
        );

        Ok(Self {
            coach,
            coachee,
            sample_rate,
        })
    }

    /// The chunk's own `device_type` decides its file. There is deliberately no
    /// branch that writes a chunk to both files or to a combined one.
    pub fn write_chunk(&mut self, chunk: &AudioChunk) -> Result<()> {
        match chunk.device_type {
            DeviceType::Microphone => self.coach.write(&chunk.data),
            DeviceType::System => self.coachee.write(&chunk.data),
        }
    }

    pub fn coach_samples(&self) -> u64 {
        self.coach.samples_written
    }

    pub fn coachee_samples(&self) -> u64 {
        self.coachee.samples_written
    }

    /// Close both files. The devices' clocks drift, so the shorter track is
    /// padded with silence to match the longer one: the two durations then
    /// agree within a frame without either track borrowing the other's audio.
    pub fn finish(mut self) -> Result<TwoTrackResult> {
        let coach = self.coach.samples_written;
        let coachee = self.coachee.samples_written;
        if coach > coachee {
            self.coachee.pad_silence(coach - coachee)?;
        } else if coachee > coach {
            self.coach.pad_silence(coachee - coach)?;
        }

        let sample_rate = self.sample_rate;
        Ok(TwoTrackResult {
            coach: self.coach.finalize(sample_rate)?,
            coachee: self.coachee.finalize(sample_rate)?,
            sample_rate,
        })
    }
}

/// 16-bit mono PCM header. Written once with a zero length, then rewritten with
/// the real one when the track closes.
fn write_wav_header<W: Write>(writer: &mut W, sample_rate: u32, samples: u64) -> Result<()> {
    let data_bytes = (samples * 2) as u32;
    let byte_rate = sample_rate * 2;

    writer.write_all(b"RIFF")?;
    writer.write_all(&(36u32 + data_bytes).to_le_bytes())?;
    writer.write_all(b"WAVE")?;
    writer.write_all(b"fmt ")?;
    writer.write_all(&16u32.to_le_bytes())?;
    writer.write_all(&1u16.to_le_bytes())?; // PCM
    writer.write_all(&1u16.to_le_bytes())?; // mono
    writer.write_all(&sample_rate.to_le_bytes())?;
    writer.write_all(&byte_rate.to_le_bytes())?;
    writer.write_all(&2u16.to_le_bytes())?; // block align
    writer.write_all(&16u16.to_le_bytes())?; // bits per sample
    writer.write_all(b"data")?;
    writer.write_all(&data_bytes.to_le_bytes())?;
    Ok(())
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

#[cfg(test)]
mod sealed_tests {
    use super::*;
    use crate::audio::recording_state::{AudioChunk, DeviceType};
    use std::sync::Arc;

    fn chunk(device_type: DeviceType, data: Vec<f32>) -> AudioChunk {
        AudioChunk {
            data,
            sample_rate: 48000,
            timestamp: 0.0,
            chunk_id: 0,
            device_type,
        }
    }

    #[test]
    fn with_the_engines_key_each_track_is_sealed_as_it_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let dek = Arc::new(SessionDek::new("sess-1", &[9u8; 32]).unwrap());
        let mut recorder = TwoTrackRecorder::with_key(
            dir.path(),
            48000,
            "Built-in Microphone",
            "BlackHole 2ch",
            Some(dek.clone()),
        )
        .unwrap();

        recorder
            .write_chunk(&chunk(DeviceType::Microphone, vec![1.0, -1.0, 0.5]))
            .unwrap();
        recorder
            .write_chunk(&chunk(DeviceType::System, vec![0.25]))
            .unwrap();
        let tracks = recorder.finish().unwrap();

        assert_eq!(
            tracks.coach.path.extension().and_then(|e| e.to_str()),
            Some(SEALED_SUFFIX),
            "a sealed track is not written to a .wav path"
        );
        let opened: Vec<u8> = crate::audio::sealed_capture::frames(&tracks.coach.path)
            .unwrap()
            .iter()
            .flat_map(|frame| dek.open(frame).unwrap())
            .collect();
        let expected: Vec<u8> = [i16::MAX, -i16::MAX, 16383i16]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        assert_eq!(opened, expected);
    }

    #[test]
    fn a_sealed_capture_leaves_no_plaintext_samples_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let dek = Arc::new(SessionDek::new("sess-1", &[9u8; 32]).unwrap());
        let mut recorder = TwoTrackRecorder::with_key(
            dir.path(),
            48000,
            "Built-in Microphone",
            "BlackHole 2ch",
            Some(dek),
        )
        .unwrap();

        // A long run of one distinctive sample value: if any of it reached the
        // file unsealed, the byte pattern would be there to find.
        recorder
            .write_chunk(&chunk(DeviceType::Microphone, vec![0.5; 4800]))
            .unwrap();
        let tracks = recorder.finish().unwrap();

        let bytes = std::fs::read(&tracks.coach.path).unwrap();
        let plaintext = 16383i16.to_le_bytes().repeat(32);
        assert!(
            !bytes.windows(plaintext.len()).any(|w| w == plaintext),
            "plaintext PCM reached a sealed track"
        );
        assert_ne!(&bytes[0..4], b"RIFF", "a sealed track is not a WAV file");
        assert!(dir.path().join("coach.wav").metadata().is_err());
    }
}
