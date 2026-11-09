use std::sync::{Arc, Mutex};
use tokio::sync::Mutex as AsyncMutex;
use anyhow::Result;
use log::{info, warn, error};
use tauri::{AppHandle, Runtime, Emitter};
use tokio::sync::mpsc;
use serde::{Serialize, Deserialize};
use std::path::PathBuf;

use super::recording_state::AudioChunk;
use super::audio_processing::create_meeting_folder;
use super::two_track::{TwoTrackRecorder, TwoTrackResult};

/// Structured transcript segment for JSON export
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptSegment {
    pub id: String,
    pub text: String,
    pub audio_start_time: f64, // Seconds from recording start
    pub audio_end_time: f64,   // Seconds from recording start
    pub duration: f64,          // Segment duration in seconds
    pub display_time: String,   // Formatted time for display like "[02:15]"
    pub confidence: f32,
    pub sequence_id: u64,
}

/// Meeting metadata structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingMetadata {
    pub version: String,
    pub meeting_id: Option<String>,
    pub meeting_name: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
    pub duration_seconds: Option<f64>,
    pub devices: DeviceInfo,
    /// The coach's track: the microphone device, nothing else on it.
    pub audio_file: String,
    /// The coachee's track: the system-audio capture, written separately so
    /// coachee audio can be deleted without touching the coach's.
    pub coachee_audio_file: String,
    pub transcript_file: String,
    pub sample_rate: u32,
    pub status: String,  // "recording", "completed", "error"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub microphone: Option<String>,
    pub system_audio: Option<String>,
}

/// Writes a recording as two unmixed tracks, one per device.
pub struct RecordingSaver {
    two_track: Option<Arc<AsyncMutex<Option<TwoTrackRecorder>>>>,
    /// Whether this recording's tracks are sealed with the session key, which
    /// decides the names the metadata carries.
    tracks_are_sealed: bool,
    mic_device_name: Option<String>,
    system_device_name: Option<String>,
    meeting_folder: Option<PathBuf>,
    meeting_name: Option<String>,
    metadata: Option<MeetingMetadata>,
    transcript_segments: Arc<Mutex<Vec<TranscriptSegment>>>,
    is_saving: Arc<Mutex<bool>>,
}

impl RecordingSaver {
    pub fn new() -> Self {
        Self {
            two_track: None,
            tracks_are_sealed: false,
            mic_device_name: None,
            system_device_name: None,
            meeting_folder: None,
            meeting_name: None,
            metadata: None,
            transcript_segments: Arc::new(Mutex::new(Vec::new())),
            is_saving: Arc::new(Mutex::new(false)),
        }
    }

    /// Set the meeting name for this recording session
    pub fn set_meeting_name(&mut self, name: Option<String>) {
        self.meeting_name = name;
    }

    /// Set device information in metadata
    pub fn set_device_info(&mut self, mic_name: Option<String>, sys_name: Option<String>) {
        // Kept as fields too: the two-track recorder needs them before the
        // meeting folder exists, and each track records the device that wrote it.
        self.mic_device_name = mic_name.clone();
        self.system_device_name = sys_name.clone();
        if let Some(ref mut metadata) = self.metadata {
            metadata.devices.microphone = mic_name;
            metadata.devices.system_audio = sys_name;

            // Write updated metadata to disk if folder exists
            if let Some(folder) = &self.meeting_folder {
                let metadata_clone = metadata.clone();
                if let Err(e) = self.write_metadata(folder, &metadata_clone) {
                    warn!("Failed to update metadata with device info: {}", e);
                }
            }
        }
    }

    /// Add or update a structured transcript segment (upserts based on sequence_id)
    /// Also saves incrementally to disk
    pub fn add_transcript_segment(&self, segment: TranscriptSegment) {
        if let Ok(mut segments) = self.transcript_segments.lock() {
            // Check if segment with same sequence_id exists (update it)
            if let Some(existing) = segments.iter_mut().find(|s| s.sequence_id == segment.sequence_id) {
                *existing = segment.clone();
                info!("Updated transcript segment {} (seq: {}) - total segments: {}",
                      segment.id, segment.sequence_id, segments.len());
            } else {
                // New segment, add it
                segments.push(segment.clone());
                info!("Added new transcript segment {} (seq: {}) - total segments: {}",
                      segment.id, segment.sequence_id, segments.len());
            }
        } else {
            error!("Failed to lock transcript segments for adding segment {}", segment.id);
        }

        // NEW: Save incrementally to disk
        if let Some(folder) = &self.meeting_folder {
            if let Err(e) = self.write_transcripts_json(folder) {
                warn!("Failed to write incremental transcript update: {}", e);
            }
        }
    }

    /// Legacy method for backward compatibility - converts text to basic segment
    pub fn add_transcript_chunk(&self, text: String) {
        let segment = TranscriptSegment {
            id: format!("seg_{}", chrono::Utc::now().timestamp_millis()),
            text,
            audio_start_time: 0.0,
            audio_end_time: 0.0,
            duration: 0.0,
            display_time: "[00:00]".to_string(),
            confidence: 1.0,
            sequence_id: 0,
        };
        self.add_transcript_segment(segment);
    }

    /// Start accumulation with optional incremental saving
    ///
    /// # Arguments
    /// * `auto_save` - If true, creates checkpoints and enables saving. If false, audio chunks are discarded.
    ///
    /// Fails, rather than recording a mixed track, when a device turns out to
    /// be a pre-mixed endpoint.
    pub fn start_accumulation(
        &mut self,
        auto_save: bool,
        mut receiver: mpsc::UnboundedReceiver<AudioChunk>,
    ) -> Result<()> {
        if auto_save {
            info!("Initializing incremental audio saver for recording (auto-save ENABLED)");
        } else {
            info!("Starting recording without audio saving (auto-save DISABLED - transcripts only)");
        }

        // Initialize meeting folder and incremental saver ONLY if auto_save is enabled
        if auto_save {
            if let Some(name) = self.meeting_name.clone() {
                // A failure here is fatal on purpose: the alternative is a
                // recording whose tracks are not separated.
                self.initialize_meeting_folder(&name, true)?;
                info!("Successfully initialized meeting folder with two unmixed tracks");
            }
        } else {
            // When auto_save is false, still create meeting folder for transcripts/metadata
            // but skip .checkpoints directory
            if let Some(name) = self.meeting_name.clone() {
                match self.initialize_meeting_folder(&name, false) {
                    Ok(()) => info!("Successfully initialized meeting folder (transcripts only)"),
                    Err(e) => {
                        error!("Failed to initialize meeting folder: {}", e);
                    }
                }
            }
        }

        // Start accumulation task
        let is_saving_clone = self.is_saving.clone();
        let two_track_arc = self.two_track.clone();
        let save_audio = auto_save;

        tokio::spawn(async move {
            info!("Recording saver accumulation task started (save_audio: {})", save_audio);

            while let Some(chunk) = receiver.recv().await {
                // Check if we should continue
                let should_continue = if let Ok(is_saving) = is_saving_clone.lock() {
                    *is_saving
                } else {
                    false
                };

                if !should_continue {
                    break;
                }

                // Only process audio chunks if auto_save is enabled
                if save_audio {
                    // The chunk's device type decides its track; nothing here
                    // combines the two.
                    if let Some(recorder_arc) = &two_track_arc {
                        let mut guard = recorder_arc.lock().await;
                        if let Some(recorder) = guard.as_mut() {
                            if let Err(e) = recorder.write_chunk(&chunk) {
                                error!("Failed to write chunk to its track: {}", e);
                            }
                        }
                    } else {
                        error!("Two-track recorder not available while accumulating");
                    }
                } else {
                    // auto_save is false: discard audio chunk (no-op)
                    // Transcription already happened in the pipeline before this point
                }
            }

            info!("Recording saver accumulation task ended");
        });

        // Set saving flag
        if let Ok(mut is_saving) = self.is_saving.lock() {
            *is_saving = true;
        }

        Ok(())
    }

    /// Initialize meeting folder structure and metadata
    ///
    /// # Arguments
    /// * `meeting_name` - Name of the meeting
    /// * `create_checkpoints` - Whether to create .checkpoints/ directory and IncrementalAudioSaver
    fn initialize_meeting_folder(&mut self, meeting_name: &str, create_checkpoints: bool) -> Result<()> {
        // Load preferences to get base recordings folder
        let base_folder = super::recording_preferences::get_default_recordings_folder();

        // Create meeting folder structure (with or without .checkpoints/ subdirectory)
        let meeting_folder = create_meeting_folder(&base_folder, meeting_name, create_checkpoints)?;

        // Only open the track files when audio is being kept at all
        if create_checkpoints {
            let mic_device = self.mic_device_name.clone().unwrap_or_default();
            let system_device = self.system_device_name.clone().unwrap_or_default();
            // The key the consent gate was issued for this session, if the
            // agent is recording under Darpana at all; frames are then sealed
            // as they are written and no plaintext audio reaches the disk.
            let key = super::darpana_client::session_key();
            let sealed = key.is_some();
            let recorder = TwoTrackRecorder::with_key(
                &meeting_folder,
                48000,
                &mic_device,
                &system_device,
                key,
            )?;
            self.two_track = Some(Arc::new(AsyncMutex::new(Some(recorder))));
            self.tracks_are_sealed = sealed;
            info!(
                "✅ Two unmixed tracks opened for meeting: {} ({})",
                meeting_name,
                if sealed { "sealed" } else { "plain" }
            );
        } else {
            info!("⚠️  No audio tracks opened (auto-save disabled)");
        }

        // Create initial metadata
        let metadata = MeetingMetadata {
            version: "1.0".to_string(),
            meeting_id: None,  // Will be set by backend
            meeting_name: Some(meeting_name.to_string()),
            created_at: chrono::Utc::now().to_rfc3339(),
            completed_at: None,
            duration_seconds: None,
            devices: DeviceInfo {
                microphone: None,  // Could be enhanced to store actual device names
                system_audio: None,
            },
            audio_file: track_file_name("coach.wav", create_checkpoints, self.tracks_are_sealed),
            coachee_audio_file: track_file_name(
                "coachee.wav",
                create_checkpoints,
                self.tracks_are_sealed,
            ),
            transcript_file: "transcripts.json".to_string(),
            sample_rate: 48000,
            status: "recording".to_string(),
        };

        // Write initial metadata.json
        self.write_metadata(&meeting_folder, &metadata)?;

        self.meeting_folder = Some(meeting_folder);
        self.metadata = Some(metadata);

        Ok(())
    }

    /// Write metadata.json to disk (atomic write with temp file)
    fn write_metadata(&self, folder: &PathBuf, metadata: &MeetingMetadata) -> Result<()> {
        let metadata_path = folder.join("metadata.json");
        let temp_path = folder.join(".metadata.json.tmp");

        let json_string = serde_json::to_string_pretty(metadata)?;
        std::fs::write(&temp_path, json_string)?;
        std::fs::rename(&temp_path, &metadata_path)?;  // Atomic

        Ok(())
    }

    /// Write transcripts.json to disk (atomic write with temp file and validation)
    fn write_transcripts_json(&self, folder: &PathBuf) -> Result<()> {
        // Clone segments to avoid holding lock during I/O
        let segments_clone = if let Ok(segments) = self.transcript_segments.lock() {
            segments.clone()
        } else {
            error!("Failed to lock transcript segments for writing");
            return Err(anyhow::anyhow!("Failed to lock transcript segments"));
        };

        info!("Writing {} transcript segments to JSON", segments_clone.len());

        let transcript_path = folder.join("transcripts.json");
        let temp_path = folder.join(".transcripts.json.tmp");

        // Create JSON structure
        let json = serde_json::json!({
            "version": "1.0",
            "segments": segments_clone,
            "last_updated": chrono::Utc::now().to_rfc3339(),
            "total_segments": segments_clone.len()
        });

        // Serialize to pretty JSON string
        let json_string = serde_json::to_string_pretty(&json)
            .map_err(|e| {
                error!("Failed to serialize transcripts to JSON: {}", e);
                anyhow::anyhow!("JSON serialization failed: {}", e)
            })?;

        // Write to temp file with error handling
        std::fs::write(&temp_path, &json_string)
            .map_err(|e| {
                error!("Failed to write transcript temp file to {}: {}", temp_path.display(), e);
                anyhow::anyhow!("Failed to write temp file: {}", e)
            })?;

        // Verify temp file was written correctly
        if !temp_path.exists() {
            error!("Temp transcript file does not exist after write: {}", temp_path.display());
            return Err(anyhow::anyhow!("Temp file verification failed"));
        }

        // Atomic rename
        std::fs::rename(&temp_path, &transcript_path)
            .map_err(|e| {
                error!("Failed to rename transcript file from {} to {}: {}",
                       temp_path.display(), transcript_path.display(), e);
                anyhow::anyhow!("Failed to rename transcript file: {}", e)
            })?;

        info!("✅ Successfully wrote transcripts.json with {} segments", segments_clone.len());
        Ok(())
    }

    // in frontend/src-tauri/src/audio/recording_saver.rs
    pub fn get_stats(&self) -> (usize, u32) {
        if let Some(ref recorder) = self.two_track {
            if let Ok(guard) = recorder.try_lock() {
                let samples = guard
                    .as_ref()
                    .map(|r| r.coach_samples().max(r.coachee_samples()) as usize)
                    .unwrap_or(0);
                (samples, 48000)
            } else {
                (0, 48000)
            }
        } else {
            (0, 48000)
        }
    }

    /// Stop and save using incremental saving approach
    ///
    /// # Arguments
    /// * `app` - Tauri app handle for emitting events
    /// * `recording_duration` - Actual recording duration in seconds (from RecordingState)
    pub async fn stop_and_save<R: Runtime>(
        &mut self,
        app: &AppHandle<R>,
        recording_duration: Option<f64>
    ) -> Result<Option<String>, String> {
        info!("Stopping recording saver");

        // Stop accumulation
        if let Ok(mut is_saving) = self.is_saving.lock() {
            *is_saving = false;
        }

        // Give time for final chunks
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        // The track recorder exists only when auto_save was enabled
        let should_save_audio = self.two_track.is_some();

        if !should_save_audio {
            info!("⚠️  No audio saver initialized (auto-save was disabled) - skipping audio finalization");
            info!("✅ Transcripts and metadata already saved incrementally");
            return Ok(None);
        }

        // Close both tracks. They are never merged: the engine reads them as
        // the coach channel and the coachee channel.
        let tracks: TwoTrackResult = if let Some(recorder_arc) = &self.two_track {
            let recorder = recorder_arc.lock().await.take();
            match recorder {
                Some(recorder) => match recorder.finish() {
                    Ok(tracks) => {
                        info!(
                            "✅ Wrote {} and {}",
                            tracks.coach.path.display(),
                            tracks.coachee.path.display()
                        );
                        tracks
                    }
                    Err(e) => {
                        error!("❌ Failed to finalize the tracks: {}", e);
                        return Err(format!("Failed to finalize audio: {}", e));
                    }
                },
                None => return Err("The tracks were already finalized".to_string()),
            }
        } else {
            error!("No track recorder initialized - cannot save recording");
            return Err("No track recorder initialized".to_string());
        };
        let final_audio_path = tracks.coach.path.clone();

        // Save final transcripts.json with validation
        if let Some(folder) = &self.meeting_folder {
            if let Err(e) = self.write_transcripts_json(folder) {
                error!("❌ Failed to write final transcripts: {}", e);
                return Err(format!("Failed to save transcripts: {}", e));
            }

            // Verify transcripts were written correctly
            let transcript_path = folder.join("transcripts.json");
            if !transcript_path.exists() {
                error!("❌ Transcript file was not created at: {}", transcript_path.display());
                return Err("Transcript file verification failed".to_string());
            }
            info!("✅ Transcripts saved and verified at: {}", transcript_path.display());
        }

        // Update metadata to completed status with actual recording duration
        if let (Some(folder), Some(mut metadata)) = (&self.meeting_folder, self.metadata.clone()) {
            metadata.status = "completed".to_string();
            metadata.completed_at = Some(chrono::Utc::now().to_rfc3339());

            // Use actual recording duration from RecordingState (more accurate than transcript segments)
            // Falls back to last transcript segment if duration not provided
            metadata.duration_seconds = recording_duration.or_else(|| {
                if let Ok(segments) = self.transcript_segments.lock() {
                    segments.last().map(|seg| seg.audio_end_time)
                } else {
                    None
                }
            });

            if let Err(e) = self.write_metadata(folder, &metadata) {
                error!("❌ Failed to update metadata to completed: {}", e);
                return Err(format!("Failed to update metadata: {}", e));
            }

            info!("✅ Metadata updated with duration: {:?}s", metadata.duration_seconds);
        }

        // Emit save event with audio and transcript paths
        let save_event = serde_json::json!({
            "audio_file": final_audio_path.to_string_lossy(),
            "coach_audio_file": tracks.coach.path.to_string_lossy(),
            "coach_device": tracks.coach.device_name,
            "coachee_audio_file": tracks.coachee.path.to_string_lossy(),
            "coachee_device": tracks.coachee.device_name,
            "transcript_file": self.meeting_folder.as_ref()
                .map(|f| f.join("transcripts.json").to_string_lossy().to_string()),
            "meeting_name": self.meeting_name,
            "meeting_folder": self.meeting_folder.as_ref()
                .map(|f| f.to_string_lossy().to_string())
        });

        if let Err(e) = app.emit("recording-saved", &save_event) {
            warn!("Failed to emit recording-saved event: {}", e);
        }

        // Clean up transcript segments
        if let Ok(mut segments) = self.transcript_segments.lock() {
            segments.clear();
        }

        Ok(Some(final_audio_path.to_string_lossy().to_string()))
    }

    /// Get the meeting folder path (for passing to backend)
    pub fn get_meeting_folder(&self) -> Option<&PathBuf> {
        self.meeting_folder.as_ref()
    }

    /// Get accumulated transcript segments (for reload sync)
    pub fn get_transcript_segments(&self) -> Vec<TranscriptSegment> {
        if let Ok(segments) = self.transcript_segments.lock() {
            segments.clone()
        } else {
            Vec::new()
        }
    }

    /// Get meeting name (for reload sync)
    pub fn get_meeting_name(&self) -> Option<String> {
        self.meeting_name.clone()
    }
}

impl Default for RecordingSaver {
    fn default() -> Self {
        Self::new()
    }
}

/// What the metadata calls a track: `coach.wav` plain, `coach.wav.enc` sealed.
fn track_file_name(name: &str, keeping_audio: bool, sealed: bool) -> String {
    match (keeping_audio, sealed) {
        (false, _) => String::new(),
        (true, false) => name.to_string(),
        (true, true) => format!("{name}.{}", super::sealed_capture::SEALED_SUFFIX),
    }
}
