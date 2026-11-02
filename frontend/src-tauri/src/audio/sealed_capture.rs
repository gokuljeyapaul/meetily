//! Captured audio is sealed as it is written, never after.
//!
//! The reference behaviour is Darpana's `engine/capture/writer.py`, and the
//! construction is the one `engine/custody/session.py` uses so the engine can
//! open what the agent wrote: AES-256-GCM under the session DEK, each frame
//! sealed as `nonce || ciphertext || tag` with a fresh 12-byte nonce and no
//! additional data, and the file a sequence of
//!
//! ```text
//! [4-byte big-endian length][sealed frame] ...
//! ```
//!
//! Two properties matter more than the format. First, there is no moment at
//! which plaintext audio exists on disk: the frame is sealed in memory and the
//! sealed bytes are the only thing written, to the final path, with no
//! temporary file to be renamed. Second, a kill mid-write is expected rather
//! than exceptional — the length prefix makes a half-written tail detectable,
//! and `frames` drops it instead of guessing at it, because a repaired frame
//! is audio nobody recorded.
//!
//! The agent never holds the engagement key. It is handed a session DEK that
//! expires, so a stolen agent decrypts one session and not an engagement.

use std::fs::{File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use anyhow::{anyhow, Context, Result};
use rand::RngCore;
use sha2::{Digest, Sha256};

/// Sealed tracks carry this suffix: nothing else is written beside them.
pub const SEALED_SUFFIX: &str = "enc";

/// The largest frame the reader will believe. A length prefix larger than this
/// means the tail is damaged, not that a huge frame follows.
pub const MAX_FRAME_BYTES: usize = 8 << 20;

const NONCE_BYTES: usize = 12;
const DEK_BYTES: usize = 32;

/// A session DEK, as the custodian issued it.
///
/// Deliberately not `Clone`: one handle, one session. `zeroize` on drop is the
/// obvious next step and belongs with the custodian wiring, not here.
pub struct SessionDek {
    cipher: Aes256Gcm,
    session_id: String,
}

impl SessionDek {
    /// Take a 32-byte DEK for one session.
    pub fn new(session_id: impl Into<String>, dek: &[u8]) -> Result<Self> {
        if dek.len() != DEK_BYTES {
            return Err(anyhow!(
                "a session DEK is {DEK_BYTES} bytes; this one is {}",
                dek.len()
            ));
        }
        Ok(Self {
            cipher: Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(dek)),
            session_id: session_id.into(),
        })
    }

    /// Which session this handle may open. Nothing else.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Seal one frame: `nonce || ciphertext || tag`.
    pub fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let mut nonce = [0u8; NONCE_BYTES];
        rand::thread_rng().fill_bytes(&mut nonce);
        let body = self
            .cipher
            .encrypt(Nonce::from_slice(&nonce), plaintext)
            .map_err(|_| anyhow!("the session DEK could not seal a captured frame"))?;
        let mut sealed = Vec::with_capacity(NONCE_BYTES + body.len());
        sealed.extend_from_slice(&nonce);
        sealed.extend_from_slice(&body);
        Ok(sealed)
    }

    /// Open one frame sealed under this session's DEK.
    pub fn open(&self, sealed: &[u8]) -> Result<Vec<u8>> {
        if sealed.len() <= NONCE_BYTES {
            return Err(anyhow!("a sealed frame is shorter than its own nonce"));
        }
        let (nonce, body) = sealed.split_at(NONCE_BYTES);
        self.cipher
            .decrypt(Nonce::from_slice(nonce), body)
            .map_err(|_| {
                anyhow!(
                    "this frame was not sealed for session {}, or it was altered",
                    self.session_id
                )
            })
    }
}

/// Writes one track, sealing every frame as it arrives.
pub struct SealedTrackWriter {
    path: PathBuf,
    file: File,
    frames_written: u64,
}

impl SealedTrackWriter {
    /// Create (or truncate) the sealed track at `path`.
    ///
    /// `path` is the final path. There is no `.partial` to rename, because a
    /// rename is a window in which something has to decide what the file is.
    pub fn create(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if path.extension().and_then(|e| e.to_str()) != Some(SEALED_SUFFIX) {
            return Err(anyhow!(
                "a sealed track is written to a .{SEALED_SUFFIX} path; refusing {}",
                path.display()
            ));
        }
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
            .with_context(|| format!("creating the sealed track {}", path.display()))?;
        Ok(Self {
            path,
            file,
            frames_written: 0,
        })
    }

    /// Seal this frame and write it, flushed before returning.
    ///
    /// Flushing every frame is the point: whatever the coach said is either
    /// sealed on disk or was never captured, with no buffered plaintext in
    /// between to be lost or leaked.
    pub fn write(&mut self, dek: &SessionDek, pcm: &[u8]) -> Result<()> {
        let sealed = dek.seal(pcm)?;
        let length =
            u32::try_from(sealed.len()).map_err(|_| anyhow!("a sealed frame cannot exceed 4 GiB"))?;
        self.file.write_all(&length.to_be_bytes())?;
        self.file.write_all(&sealed)?;
        self.file.flush()?;
        self.frames_written += 1;
        Ok(())
    }

    /// How many frames have been sealed to this track.
    pub fn frames_written(&self) -> u64 {
        self.frames_written
    }

    /// Where the track is.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Read back the sealed frames of a track, dropping a damaged tail.
///
/// A truncated final frame — the ordinary outcome of a kill mid-write — ends
/// the iteration. It is never repaired, padded or partially decrypted.
pub fn frames(path: impl AsRef<Path>) -> Result<Vec<Vec<u8>>> {
    let mut reader = BufReader::new(File::open(path.as_ref())?);
    let mut out = Vec::new();
    loop {
        let mut header = [0u8; 4];
        match reader.read_exact(&mut header) {
            Ok(()) => {}
            Err(_) => return Ok(out),
        }
        let length = u32::from_be_bytes(header) as usize;
        if length == 0 || length > MAX_FRAME_BYTES {
            return Ok(out);
        }
        let mut frame = vec![0u8; length];
        if reader.read_exact(&mut frame).is_err() {
            return Ok(out);
        }
        out.push(frame);
    }
}

/// The SHA-256 of a file, lowercase hex, read in chunks.
pub fn file_digest(path: impl AsRef<Path>) -> Result<String> {
    let mut file = File::open(path.as_ref())?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// The SHA-256 of some bytes, lowercase hex.
pub fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, SeekFrom};

    fn dek(session: &str, byte: u8) -> SessionDek {
        SessionDek::new(session, &[byte; DEK_BYTES]).unwrap()
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("darpana-sealed-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Recognisable "audio": nothing like it may appear on disk.
    fn pcm(frame: u8) -> Vec<u8> {
        let mut bytes = vec![frame; 64];
        bytes.extend_from_slice(b"COACH-SPEECH-PLAINTEXT");
        bytes
    }

    #[test]
    fn no_plaintext_exists_on_disk_at_any_point_during_capture() {
        let dir = temp_dir("plaintext");
        let dek = dek("session-a", 7);
        let mut writer = SealedTrackWriter::create(dir.join("coach.enc")).unwrap();

        for frame in 0..5u8 {
            let plaintext = pcm(frame);
            writer.write(&dek, &plaintext).unwrap();

            // After every frame, the whole directory is searched for the
            // plaintext marker — not just the track, in case something
            // helpful wrote a temporary file beside it.
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                let bytes = std::fs::read(&path).unwrap();
                assert!(
                    !contains(&bytes, b"COACH-SPEECH-PLAINTEXT"),
                    "{} holds plaintext audio",
                    path.display()
                );
                assert_eq!(
                    path.extension().and_then(|e| e.to_str()),
                    Some(SEALED_SUFFIX),
                    "{} is not a sealed track",
                    path.display()
                );
            }
        }
    }

    #[test]
    fn a_kill_mid_capture_leaves_sealed_frames_and_no_plaintext() {
        let dir = temp_dir("kill");
        let dek = dek("session-a", 7);
        {
            let mut writer = SealedTrackWriter::create(dir.join("coach.enc")).unwrap();
            for frame in 0..3u8 {
                writer.write(&dek, &pcm(frame)).unwrap();
            }
            // No close, no finish: the process simply ends here.
            std::mem::forget(writer);
        }

        let read_back = frames(dir.join("coach.enc")).unwrap();
        assert_eq!(read_back.len(), 3, "every flushed frame survived the kill");
        for (frame, sealed) in read_back.iter().enumerate() {
            assert_eq!(dek.open(sealed).unwrap(), pcm(frame as u8));
        }
    }

    #[test]
    fn a_truncated_final_frame_is_dropped_not_repaired() {
        let dir = temp_dir("truncated");
        let dek = dek("session-a", 7);
        let path = dir.join("coach.enc");
        let mut writer = SealedTrackWriter::create(&path).unwrap();
        for frame in 0..3u8 {
            writer.write(&dek, &pcm(frame)).unwrap();
        }
        drop(writer);

        // Lose the last nine bytes, as a kill between two writes would.
        let full = std::fs::metadata(&path).unwrap().len();
        let mut file = OpenOptions::new().write(true).open(&path).unwrap();
        file.set_len(full - 9).unwrap();
        file.seek(SeekFrom::End(0)).unwrap();
        drop(file);

        let read_back = frames(&path).unwrap();
        assert_eq!(
            read_back.len(),
            2,
            "the damaged frame is dropped, and the intact ones are not"
        );
        assert_eq!(dek.open(&read_back[0]).unwrap(), pcm(0));
        assert_eq!(dek.open(&read_back[1]).unwrap(), pcm(1));
    }

    #[test]
    fn one_session_cannot_open_another_sessions_track() {
        let dir = temp_dir("cross");
        let mine = dek("session-a", 7);
        let theirs = dek("session-b", 9);
        let path = dir.join("coach.enc");
        let mut writer = SealedTrackWriter::create(&path).unwrap();
        writer.write(&mine, &pcm(0)).unwrap();
        drop(writer);

        let sealed = &frames(&path).unwrap()[0];
        assert!(
            theirs.open(sealed).is_err(),
            "a DEK for another session must not open this frame"
        );
        assert_eq!(mine.open(sealed).unwrap(), pcm(0));
    }

    #[test]
    fn a_track_is_only_written_to_a_sealed_path() {
        let dir = temp_dir("suffix");
        assert!(
            SealedTrackWriter::create(dir.join("coach.wav")).is_err(),
            "a plaintext-looking path must be refused rather than written"
        );
        assert!(!dir.join("coach.wav").exists());
    }

    #[test]
    fn a_dek_of_the_wrong_length_is_refused() {
        assert!(SessionDek::new("session-a", &[0u8; 16]).is_err());
        assert!(SessionDek::new("session-a", &[0u8; 32]).is_ok());
    }

    #[test]
    fn the_sealed_frame_layout_is_the_engines_layout() {
        // nonce || ciphertext || tag, 12 + len + 16, as
        // engine/custody/session.py writes it.
        let dek = dek("session-a", 7);
        let sealed = dek.seal(&[1u8; 100]).unwrap();
        assert_eq!(sealed.len(), NONCE_BYTES + 100 + 16);
        assert_eq!(dek.open(&sealed).unwrap(), vec![1u8; 100]);
    }

    #[test]
    fn digests_are_lowercase_hex_sha256_of_the_sealed_bytes() {
        let dir = temp_dir("digest");
        let path = dir.join("coach.enc");
        let mut writer = SealedTrackWriter::create(&path).unwrap();
        writer.write(&dek("session-a", 7), &pcm(0)).unwrap();
        drop(writer);

        let on_disk = std::fs::read(&path).unwrap();
        assert_eq!(file_digest(&path).unwrap(), digest(&on_disk));
        assert_eq!(file_digest(&path).unwrap().len(), 64);
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    }
}

/// Emits a sealed track for Darpana itself to open, which is the only proof
/// that matters for the format: the agent writing bytes only the agent can read
/// would satisfy every test above and still be useless.
///
/// Ignored by default because the check spans two languages — run
/// `cargo test --lib -- --ignored interop_fixture`, then open
/// `/tmp/darpana-interop/coach.enc` with `engine.custody.session` (or AESGCM
/// directly) under a DEK of 32 `0x07` bytes and expect `frame-one`,
/// `frame-two`.
#[cfg(test)]
mod interop_fixture {
    use super::*;
    #[test]
    #[ignore]
    fn emit_a_sealed_track_for_the_engine_to_open() {
        let dir = std::path::PathBuf::from("/tmp/darpana-interop");
        std::fs::create_dir_all(&dir).unwrap();
        let dek = SessionDek::new("session-a", &[7u8; 32]).unwrap();
        let mut writer = SealedTrackWriter::create(dir.join("coach.enc")).unwrap();
        writer.write(&dek, b"frame-one").unwrap();
        writer.write(&dek, b"frame-two").unwrap();
    }
}
