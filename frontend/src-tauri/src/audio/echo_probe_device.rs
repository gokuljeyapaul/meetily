//! The echo gate's probe, played on real devices.
//!
//! `echo_gate.rs` decides what the measurement means; this module is the part
//! that drives hardware, kept separate so the decision can be tested without a
//! sound card and so a device failure here becomes a refusal there.
//!
//! Both streams are opened with cpal, the same way the rest of the capture
//! code does: the microphone is recorded with the output silent (the floor),
//! then recorded again while the probe plays. No real device is exercised in
//! CI, so bugs here surface as a refusal — which is the safe direction — and
//! the card's real-device evidence comes from a machine with a microphone.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use log::info;

use super::echo_gate::{ProbeCapture, ProbePlayer, PROBE_DURATION_MS};

/// Plays the probe on the default output device and records the default input.
pub struct DeviceProbePlayer {
    output_name: String,
}

impl DeviceProbePlayer {
    /// Fails when there is no output device to test, which the gate treats as
    /// a refusal rather than a pass.
    pub fn for_default_output() -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| anyhow!("no default output device"))?;
        Ok(Self {
            output_name: device.name().unwrap_or_else(|_| "output device".to_string()),
        })
    }

    fn record_microphone(&self, duration: Duration, probe: Option<&[f32]>) -> Result<Vec<f32>> {
        let host = cpal::default_host();
        let input = host
            .default_input_device()
            .ok_or_else(|| anyhow!("no default input device"))?;
        let input_config = input.default_input_config()?;
        let channels = input_config.channels() as usize;

        let captured = Arc::new(Mutex::new(Vec::<f32>::new()));
        let sink = captured.clone();
        let input_stream = input.build_input_stream(
            &input_config.clone().into(),
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                if let Ok(mut buffer) = sink.lock() {
                    // First channel only: the probe is one tone and the gate
                    // measures one microphone.
                    buffer.extend(data.iter().step_by(channels.max(1)));
                }
            },
            |e| info!("echo probe input stream error: {e}"),
            None,
        )?;
        input_stream.play()?;

        // The probe stream is built only for the second window, so the first
        // measures the room with the output genuinely silent.
        let _output_stream = match probe {
            None => None,
            Some(probe) => {
                let output = host
                    .default_output_device()
                    .ok_or_else(|| anyhow!("no default output device"))?;
                let output_config = output.default_output_config()?;
                let out_channels = output_config.channels() as usize;
                let probe = probe.to_vec();
                let mut cursor = 0usize;
                let stream = output.build_output_stream(
                    &output_config.clone().into(),
                    move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                        for frame in data.chunks_mut(out_channels.max(1)) {
                            let sample = probe.get(cursor).copied().unwrap_or(0.0);
                            cursor += 1;
                            for slot in frame.iter_mut() {
                                *slot = sample;
                            }
                        }
                    },
                    |e| info!("echo probe output stream error: {e}"),
                    None,
                )?;
                stream.play()?;
                Some(stream)
            }
        };

        std::thread::sleep(duration);
        drop(input_stream);

        let samples = captured
            .lock()
            .map_err(|_| anyhow!("the probe capture buffer was poisoned"))?
            .clone();
        Ok(samples)
    }
}

impl ProbePlayer for DeviceProbePlayer {
    fn output_device_name(&self) -> String {
        self.output_name.clone()
    }

    fn play_probe(&mut self, probe: &[f32], _sample_rate: u32) -> Result<ProbeCapture> {
        // A little longer than the probe, so the capture cannot end before the
        // tone has finished arriving.
        let window = Duration::from_millis(PROBE_DURATION_MS + 50);
        let floor = self.record_microphone(window, None)?;
        let during_probe = self.record_microphone(window, Some(probe))?;
        Ok(ProbeCapture {
            floor,
            during_probe,
        })
    }
}
