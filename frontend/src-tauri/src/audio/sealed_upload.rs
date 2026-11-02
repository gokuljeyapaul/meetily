//! The agent's half of a resumable upload, and what it does after a crash.
//!
//! Darpana's `engine/capture/upload.py` is the server's half: it verifies every
//! chunk against the declared digest and the assembled object against a
//! whole-object digest, and names the chunk it rejected so one chunk is
//! retried rather than the session. This module is what the agent has to do for
//! that to be possible — declare the digests before sending anything, send what
//! the server says is missing, and never consider an interrupted upload done.
//!
//! `recover_or_discard` is the startup counterpart of
//! `engine/capture/recovery.py`: a capture found mid-flight is judged against
//! its own manifest, and a track that is missing, the wrong length, or the
//! wrong content means the session is discarded with a reason — never repaired,
//! because a repaired track is audio nobody recorded, and an analysis citing it
//! would cite something that did not happen.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use sha2::{Digest, Sha256};

use super::sealed_capture::file_digest;

/// What the agent declares before it sends a byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadManifest {
    /// Which session this object belongs to.
    pub session_id: String,
    /// The object's name on the server.
    pub object_name: String,
    /// One digest per chunk, in order.
    pub chunk_digests: Vec<String>,
    /// The digest of the whole assembled object.
    pub object_digest: String,
    /// The chunk size used to cut the file.
    pub chunk_bytes: usize,
}

impl UploadManifest {
    /// Read a sealed track and declare what the server should expect.
    pub fn for_file(
        session_id: impl Into<String>,
        path: impl AsRef<Path>,
        chunk_bytes: usize,
    ) -> Result<Self> {
        if chunk_bytes == 0 {
            return Err(anyhow!("a chunk cannot be zero bytes"));
        }
        let path = path.as_ref();
        let object_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| anyhow!("{} has no file name", path.display()))?
            .to_string();

        let mut file = File::open(path)?;
        let mut chunk_digests = Vec::new();
        let mut whole = Sha256::new();
        let mut buffer = vec![0u8; chunk_bytes];
        loop {
            let mut filled = 0;
            while filled < chunk_bytes {
                let read = file.read(&mut buffer[filled..])?;
                if read == 0 {
                    break;
                }
                filled += read;
            }
            if filled == 0 {
                break;
            }
            chunk_digests.push(hex::encode(Sha256::digest(&buffer[..filled])));
            whole.update(&buffer[..filled]);
            if filled < chunk_bytes {
                break;
            }
        }

        Ok(Self {
            session_id: session_id.into(),
            object_name,
            chunk_digests,
            object_digest: hex::encode(whole.finalize()),
            chunk_bytes,
        })
    }

    /// How many chunks the object has.
    pub fn chunk_count(&self) -> usize {
        self.chunk_digests.len()
    }

    /// Read one chunk back off disk, for a first send or a retry.
    pub fn read_chunk(&self, path: impl AsRef<Path>, index: usize) -> Result<Vec<u8>> {
        if index >= self.chunk_count() {
            return Err(anyhow!(
                "chunk {index} is not in this manifest; it declares {}",
                self.chunk_count()
            ));
        }
        use std::io::{Seek, SeekFrom};
        let mut file = File::open(path.as_ref())?;
        file.seek(SeekFrom::Start((index * self.chunk_bytes) as u64))?;
        let mut buffer = vec![0u8; self.chunk_bytes];
        let mut filled = 0;
        while filled < self.chunk_bytes {
            let read = file.read(&mut buffer[filled..])?;
            if read == 0 {
                break;
            }
            filled += read;
        }
        buffer.truncate(filled);

        let actual = hex::encode(Sha256::digest(&buffer));
        if actual != self.chunk_digests[index] {
            return Err(anyhow!(
                "chunk {index} no longer matches the digest declared for it \
                 ({} declared, {actual} on disk): the track changed under the upload",
                self.chunk_digests[index]
            ));
        }
        Ok(buffer)
    }

    /// Which chunks still have to be sent, given what the server acknowledged.
    ///
    /// The server is the authority on what it holds. The agent does not
    /// remember "we got to chunk 40" across a crash, because that memory is
    /// exactly what a crash invalidates.
    pub fn still_to_send(&self, server_has: &[usize]) -> Vec<usize> {
        (0..self.chunk_count())
            .filter(|index| !server_has.contains(index))
            .collect()
    }

    /// Whether this upload may be called done.
    ///
    /// Only the server's acknowledgement of every chunk counts; the agent
    /// having sent them does not.
    pub fn complete(&self, server_has: &[usize]) -> bool {
        !self.chunk_digests.is_empty() && self.still_to_send(server_has).is_empty()
    }
}

/// One track as the manifest declared it, for the crash check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackManifest {
    /// The file name, relative to the capture directory.
    pub name: String,
    /// How many bytes it should be.
    pub size_bytes: u64,
    /// The digest it should have.
    pub digest: String,
}

/// Why a recovered capture was discarded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscardReason {
    /// The declared track is not on disk.
    TrackMissing,
    /// The track is on disk but not the length it should be.
    TrackTruncated,
    /// The track is the right length and the wrong content.
    DigestMismatch,
}

impl DiscardReason {
    /// The ledger's wording for this reason.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TrackMissing => "track_missing",
            Self::TrackTruncated => "track_truncated",
            Self::DigestMismatch => "digest_mismatch",
        }
    }
}

/// What startup decided about a capture it found mid-flight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recovery {
    /// Every track matches: the upload picks up where the server left off.
    Resume {
        /// The state the session moves to, in the engine's vocabulary.
        next_state: &'static str,
        /// What goes in the ledger.
        ledger_entry: String,
    },
    /// Something does not match: the session never enters the pipeline.
    Discard {
        /// The state the session moves to, in the engine's vocabulary.
        next_state: &'static str,
        /// Why.
        reason: DiscardReason,
        /// What goes in the ledger.
        ledger_entry: String,
        /// The sealed track files to destroy.
        destroy_local_ciphertext: Vec<PathBuf>,
        /// Whether the session DEK handle goes with them. It does.
        destroy_dek_handle: bool,
    },
}

/// Judge a capture found at startup against its manifest.
pub fn recover_or_discard(directory: impl AsRef<Path>, manifest: &[TrackManifest]) -> Recovery {
    let directory = directory.as_ref();
    let all_tracks = || -> Vec<PathBuf> {
        manifest
            .iter()
            .map(|track| directory.join(&track.name))
            .collect()
    };
    let discard = |track: &TrackManifest, reason: DiscardReason, because: String| Recovery::Discard {
        next_state: "discarded",
        reason,
        ledger_entry: format!(
            "capture discarded: the track {} {because}, and a track is never \
             repaired — an analysis citing a repaired track would cite audio \
             nobody recorded",
            track.name
        ),
        destroy_local_ciphertext: all_tracks(),
        destroy_dek_handle: true,
    };

    for track in manifest {
        let path = directory.join(&track.name);
        if !path.exists() {
            return discard(track, DiscardReason::TrackMissing, "is not on disk".into());
        }
        let size = match std::fs::metadata(&path) {
            Ok(metadata) => metadata.len(),
            Err(_) => {
                return discard(track, DiscardReason::TrackMissing, "cannot be read".into());
            }
        };
        if size != track.size_bytes {
            return discard(
                track,
                DiscardReason::TrackTruncated,
                format!(
                    "is {size} bytes where the manifest declares {}",
                    track.size_bytes
                ),
            );
        }
        match file_digest(&path) {
            Ok(digest) if digest == track.digest => {}
            Ok(digest) => {
                return discard(
                    track,
                    DiscardReason::DigestMismatch,
                    format!(
                        "digests to {digest} where the manifest declares {}",
                        track.digest
                    ),
                );
            }
            Err(_) => {
                return discard(track, DiscardReason::TrackMissing, "cannot be read".into());
            }
        }
    }

    Recovery::Resume {
        next_state: "uploading",
        ledger_entry: "capture recovered: every declared track matches its size and \
                       digest, so the upload resumes"
            .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::sealed_capture::{SealedTrackWriter, SessionDek};
    use super::super::session_lifecycle::Lifecycle;
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("darpana-upload-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sealed_track(dir: &Path, name: &str, frames: u8) -> PathBuf {
        let dek = SessionDek::new("session-a", &[7u8; 32]).unwrap();
        let path = dir.join(name);
        let mut writer = SealedTrackWriter::create(&path).unwrap();
        for frame in 0..frames {
            writer.write(&dek, &vec![frame; 500]).unwrap();
        }
        path
    }

    fn manifest_of(path: &Path, name: &str) -> TrackManifest {
        TrackManifest {
            name: name.to_string(),
            size_bytes: std::fs::metadata(path).unwrap().len(),
            digest: file_digest(path).unwrap(),
        }
    }

    #[test]
    fn the_manifest_declares_a_digest_per_chunk_and_one_for_the_object() {
        let dir = temp_dir("manifest");
        let path = sealed_track(&dir, "coach.enc", 10);
        let manifest = UploadManifest::for_file("session-a", &path, 1024).unwrap();

        let bytes = std::fs::read(&path).unwrap();
        let expected_chunks = (bytes.len() + 1023) / 1024;
        assert_eq!(manifest.chunk_count(), expected_chunks);
        assert_eq!(manifest.object_digest, file_digest(&path).unwrap());
        for (index, chunk) in bytes.chunks(1024).enumerate() {
            assert_eq!(
                manifest.chunk_digests[index],
                hex::encode(Sha256::digest(chunk)),
                "chunk {index}"
            );
        }
    }

    #[test]
    fn an_interrupted_upload_is_never_complete_and_resumes_from_the_server() {
        let dir = temp_dir("resume");
        let path = sealed_track(&dir, "coach.enc", 10);
        let manifest = UploadManifest::for_file("session-a", &path, 1024).unwrap();

        let acknowledged: Vec<usize> = (0..manifest.chunk_count() - 2).collect();
        assert!(!manifest.complete(&acknowledged));
        assert_eq!(
            manifest.still_to_send(&acknowledged),
            vec![manifest.chunk_count() - 2, manifest.chunk_count() - 1],
            "only what the server is missing is sent again"
        );

        let everything: Vec<usize> = (0..manifest.chunk_count()).collect();
        assert!(manifest.complete(&everything));
    }

    #[test]
    fn a_chunk_is_read_back_exactly_as_it_was_declared() {
        let dir = temp_dir("chunk");
        let path = sealed_track(&dir, "coach.enc", 6);
        let manifest = UploadManifest::for_file("session-a", &path, 512).unwrap();

        for index in 0..manifest.chunk_count() {
            let chunk = manifest.read_chunk(&path, index).unwrap();
            assert_eq!(
                hex::encode(Sha256::digest(&chunk)),
                manifest.chunk_digests[index]
            );
        }
        assert!(manifest.read_chunk(&path, manifest.chunk_count()).is_err());
    }

    #[test]
    fn a_track_that_changed_under_the_upload_refuses_to_be_sent() {
        let dir = temp_dir("changed");
        let path = sealed_track(&dir, "coach.enc", 6);
        let manifest = UploadManifest::for_file("session-a", &path, 512).unwrap();

        let mut bytes = std::fs::read(&path).unwrap();
        bytes[10] ^= 0xff;
        std::fs::write(&path, &bytes).unwrap();

        let error = manifest.read_chunk(&path, 0).unwrap_err().to_string();
        assert!(error.contains("chunk 0"), "the chunk is named: {error}");
    }

    #[test]
    fn a_whole_capture_that_matches_its_manifest_resumes() {
        let dir = temp_dir("recover-ok");
        let coach = sealed_track(&dir, "coach.enc", 4);
        let coachee = sealed_track(&dir, "coachee.enc", 4);
        let manifest = vec![
            manifest_of(&coach, "coach.enc"),
            manifest_of(&coachee, "coachee.enc"),
        ];

        match recover_or_discard(&dir, &manifest) {
            Recovery::Resume { next_state, .. } => assert_eq!(next_state, "uploading"),
            other => panic!("expected a resume, got {other:?}"),
        }
        assert!(coach.exists() && coachee.exists());
    }

    #[test]
    fn a_truncated_track_is_discarded_and_left_alone() {
        let dir = temp_dir("recover-truncated");
        let coach = sealed_track(&dir, "coach.enc", 4);
        let manifest = vec![manifest_of(&coach, "coach.enc")];

        let before = std::fs::read(&coach).unwrap();
        let file = std::fs::OpenOptions::new().write(true).open(&coach).unwrap();
        file.set_len(before.len() as u64 - 20).unwrap();
        drop(file);

        match recover_or_discard(&dir, &manifest) {
            Recovery::Discard {
                reason,
                next_state,
                destroy_dek_handle,
                destroy_local_ciphertext,
                ledger_entry,
            } => {
                assert_eq!(reason, DiscardReason::TrackTruncated);
                assert_eq!(next_state, "discarded");
                assert!(destroy_dek_handle);
                assert_eq!(destroy_local_ciphertext, vec![coach.clone()]);
                assert!(ledger_entry.contains("never repaired"));
            }
            other => panic!("expected a discard, got {other:?}"),
        }
        assert_eq!(
            std::fs::read(&coach).unwrap(),
            before[..before.len() - 20],
            "recovery judges the track; it does not touch it"
        );
    }

    #[test]
    fn the_right_length_and_the_wrong_content_is_still_discarded() {
        let dir = temp_dir("recover-content");
        let coach = sealed_track(&dir, "coach.enc", 4);
        let manifest = vec![manifest_of(&coach, "coach.enc")];

        let mut bytes = std::fs::read(&coach).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        std::fs::write(&coach, &bytes).unwrap();

        match recover_or_discard(&dir, &manifest) {
            Recovery::Discard { reason, .. } => assert_eq!(reason, DiscardReason::DigestMismatch),
            other => panic!("expected a discard, got {other:?}"),
        }
    }

    #[test]
    fn a_missing_track_is_discarded() {
        let dir = temp_dir("recover-missing");
        let coach = sealed_track(&dir, "coach.enc", 4);
        let manifest = vec![
            manifest_of(&coach, "coach.enc"),
            TrackManifest {
                name: "coachee.enc".into(),
                size_bytes: 1234,
                digest: "0".repeat(64),
            },
        ];

        match recover_or_discard(&dir, &manifest) {
            Recovery::Discard { reason, .. } => assert_eq!(reason, DiscardReason::TrackMissing),
            other => panic!("expected a discard, got {other:?}"),
        }
    }

    #[test]
    fn every_recovery_outcome_is_a_state_the_engine_recognises() {
        let dir = temp_dir("recover-states");
        let coach = sealed_track(&dir, "coach.enc", 2);
        let lifecycle = Lifecycle::vendored();

        for manifest in [
            vec![manifest_of(&coach, "coach.enc")],
            vec![TrackManifest {
                name: "gone.enc".into(),
                size_bytes: 1,
                digest: "0".repeat(64),
            }],
        ] {
            let state = match recover_or_discard(&dir, &manifest) {
                Recovery::Resume { next_state, .. } => next_state,
                Recovery::Discard { next_state, .. } => next_state,
            };
            assert!(
                lifecycle.states().contains(state),
                "{state} is not a state Darpana recognises"
            );
        }
    }
}
