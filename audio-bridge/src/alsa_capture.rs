use alsa::pcm::{PCM, HwParams, Access, Format, State};
use alsa::{Direction, ValueOr};
use log::{info, warn, error, debug};
use std::sync::{Arc, Mutex};
use crate::types::{Config, FormatDescriptor, Encoding, VALID_PCM_SAMPLE_RATES, VALID_BIT_DEPTHS};
use crate::backpressure::BackpressureController;

/// Number of frames per read period
const PERIOD_FRAMES: u32 = 1024;

/// Reads PCM/DSD frames from the ALSA loopback capture device
pub struct AlsaCaptureReader {
    config: Config,
    pcm: Option<PCM>,
    current_format: Option<FormatDescriptor>,
    backpressure: Arc<Mutex<BackpressureController>>,
}

impl AlsaCaptureReader {
    pub fn new(config: Config, backpressure: Arc<Mutex<BackpressureController>>) -> Self {
        AlsaCaptureReader {
            config,
            pcm: None,
            current_format: None,
            backpressure,
        }
    }

    /// Open the ALSA capture device
    pub fn open(&mut self) -> Result<(), String> {
        info!("Opening ALSA capture device: {}", self.config.alsa_device);
        let pcm = PCM::new(&self.config.alsa_device, Direction::Capture, false)
            .map_err(|e| format!("Failed to open ALSA device '{}': {} (code: {})", self.config.alsa_device, e, e.errno()))?;

        // Configure hardware parameters
        {
            let hwp = HwParams::any(&pcm)
                .map_err(|e| format!("Failed to get hw params: {} (code: {})", e, e.errno()))?;

            hwp.set_channels(2)
                .map_err(|e| format!("Failed to set channels: {} (code: {})", e, e.errno()))?;
            hwp.set_rate(44100, ValueOr::Nearest)
                .map_err(|e| format!("Failed to set rate: {} (code: {})", e, e.errno()))?;
            hwp.set_format(Format::s16_le())
                .map_err(|e| format!("Failed to set format: {} (code: {})", e, e.errno()))?;
            hwp.set_access(Access::RWInterleaved)
                .map_err(|e| format!("Failed to set access: {} (code: {})", e, e.errno()))?;
            hwp.set_period_size(PERIOD_FRAMES as i64, ValueOr::Nearest)
                .map_err(|e| format!("Failed to set period size: {} (code: {})", e, e.errno()))?;

            pcm.hw_params(&hwp)
                .map_err(|e| format!("Failed to apply hw params: {} (code: {})", e, e.errno()))?;
        }

        pcm.start()
            .map_err(|e| format!("Failed to start PCM capture: {} (code: {})", e, e.errno()))?;

        self.current_format = Some(self.detect_format(&pcm)?);
        self.pcm = Some(pcm);
        info!("ALSA capture device opened successfully");
        Ok(())
    }

    /// Close the ALSA capture device
    pub fn close(&mut self) {
        if let Some(pcm) = self.pcm.take() {
            let _ = pcm.drop();
            info!("ALSA capture device closed");
        }
    }

    /// Read a period of frames. Returns (bytes, format_changed, new_format).
    /// Handles overrun recovery internally.
    pub fn read_frames(&mut self) -> Result<(Vec<u8>, bool, Option<FormatDescriptor>), String> {
        let pcm = match &self.pcm {
            Some(p) => p,
            None => return Err("ALSA device not open".to_string()),
        };

        // Check backpressure
        if let Ok(bp) = self.backpressure.lock() {
            if bp.is_paused() {
                return Ok((vec![], false, None));
            }
        }

        // 2 channels × 2 bytes (S16_LE) × PERIOD_FRAMES
        let frame_bytes = 2 * 2 * PERIOD_FRAMES as usize;
        let mut buf = vec![0i16; 2 * PERIOD_FRAMES as usize];

        let io = pcm.io_i16().map_err(|e| format!("Failed to get PCM IO: {}", e))?;

        match io.readi(&mut buf) {
            Ok(frames) => {
                let byte_count = frames * 2 * 2; // frames * channels * bytes_per_sample
                let bytes: Vec<u8> = buf[..frames * 2]
                    .iter()
                    .flat_map(|s| s.to_le_bytes())
                    .collect();

                // Detect format changes
                let new_fmt = self.detect_format(pcm).ok();
                let format_changed = match (&self.current_format, &new_fmt) {
                    (Some(old), Some(new)) => old != new,
                    (None, Some(_)) => true,
                    _ => false,
                };

                if format_changed {
                    if let Some(ref fmt) = new_fmt {
                        debug!("Format change detected: {:?}", fmt);
                        self.current_format = new_fmt.clone();
                    }
                }

                Ok((bytes, format_changed, if format_changed { new_fmt } else { None }))
            }
            Err(e) if e.errno() == nix::errno::Errno::EPIPE => {
                // Overrun: recover via snd_pcm_prepare
                warn!("ALSA overrun (EPIPE) detected; recovering via snd_pcm_prepare");
                pcm.prepare()
                    .map_err(|e2| format!("Failed to recover from overrun: {} (code: {})", e2, e2.errno()))?;
                Ok((vec![], false, None))
            }
            Err(e) => {
                error!("ALSA read error: {} (code: {})", e, e.errno());
                Err(format!("ALSA read error: {} (code: {})", e, e.errno()))
            }
        }
    }

    /// Detect the current format from ALSA hw_params
    fn detect_format(&self, pcm: &PCM) -> Result<FormatDescriptor, String> {
        let hwp = pcm.hw_params_current()
            .map_err(|e| format!("Failed to get current hw params: {}", e))?;

        let rate = hwp.get_rate()
            .map_err(|e| format!("Failed to get sample rate: {}", e))?;
        let channels = hwp.get_channels()
            .map_err(|e| format!("Failed to get channels: {}", e))? as u8;
        let format = hwp.get_format()
            .map_err(|e| format!("Failed to get format: {}", e))?;

        let bit_depth: u8 = match format {
            Format::S16LE | Format::S16BE => 16,
            Format::S24LE | Format::S24BE | Format::S24_3LE | Format::S24_3BE => 24,
            Format::S32LE | Format::S32BE => 32,
            _ => {
                warn!("Unknown ALSA format {:?}; defaulting to 16-bit", format);
                16
            }
        };

        Ok(FormatDescriptor {
            encoding: Encoding::PCM,
            sample_rate: rate,
            bit_depth,
            channels,
            dsd_rate: None,
        })
    }

    /// Get the current format descriptor
    pub fn current_format(&self) -> Option<&FormatDescriptor> {
        self.current_format.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Encoding, FormatDescriptor};

    #[test]
    fn test_format_descriptor_equality() {
        let fmt1 = FormatDescriptor {
            encoding: Encoding::PCM,
            sample_rate: 44100,
            bit_depth: 16,
            channels: 2,
            dsd_rate: None,
        };
        let fmt2 = fmt1.clone();
        assert_eq!(fmt1, fmt2);
    }

    #[test]
    fn test_format_descriptor_inequality_on_rate_change() {
        let fmt1 = FormatDescriptor {
            encoding: Encoding::PCM,
            sample_rate: 44100,
            bit_depth: 16,
            channels: 2,
            dsd_rate: None,
        };
        let fmt2 = FormatDescriptor {
            encoding: Encoding::PCM,
            sample_rate: 96000,
            bit_depth: 16,
            channels: 2,
            dsd_rate: None,
        };
        assert_ne!(fmt1, fmt2);
    }
}
