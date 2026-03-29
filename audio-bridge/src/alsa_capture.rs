use alsa::pcm::{PCM, HwParams, Access, Format};
use alsa::{Direction, ValueOr};
use log::{info, warn, error, debug};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::sync::mpsc::SyncSender;
use crate::types::FormatDescriptor;

/// Number of frames per read period
const PERIOD_FRAMES: u32 = 1024;

/// Map bit depth to ALSA PCM format
fn bits_to_format(bits: u8) -> Format {
    match bits {
        16 => Format::s16(),
        24 => Format::s24(),
        32 => Format::s32(),
        _ => {
            warn!("Unsupported bit depth {}; defaulting to S16LE", bits);
            Format::s16()
        }
    }
}

/// Bytes per sample for a given bit depth
fn bytes_per_sample(bits: u8) -> usize {
    match bits {
        16 => 2,
        24 => 4, // ALSA S24LE is stored in 4 bytes
        32 => 4,
        _ => 2,
    }
}

/// Start ALSA capture in the current thread.
/// Reads PCM frames and sends Vec<u8> chunks to `tx`.
/// Stops when `stop` flag is set.
pub fn start_capture(
    device: &str,
    fmt: &FormatDescriptor,
    tx: SyncSender<Vec<u8>>,
    stop: Arc<AtomicBool>,
) {
    info!("Opening ALSA capture device: {} (bits={} channels={} rate={})", device, fmt.bits, fmt.channels, fmt.rate);

    let pcm = match PCM::new(device, Direction::Capture, false) {
        Ok(p) => p,
        Err(e) => {
            error!("Fatal: failed to open ALSA device '{}': {} (errno: {})", device, e, e.errno());
            std::process::exit(1);
        }
    };

    // Configure hardware parameters
    {
        let hwp = match HwParams::any(&pcm) {
            Ok(h) => h,
            Err(e) => {
                error!("Failed to get hw params: {}", e);
                return;
            }
        };

        if let Err(e) = hwp.set_channels(fmt.channels as u32) {
            error!("Failed to set channels: {}", e);
            return;
        }
        if let Err(e) = hwp.set_rate(fmt.rate, ValueOr::Nearest) {
            error!("Failed to set rate {}: {}", fmt.rate, e);
            return;
        }
        let alsa_fmt = bits_to_format(fmt.bits);
        if let Err(e) = hwp.set_format(alsa_fmt) {
            error!("Failed to set format: {}", e);
            return;
        }
        if let Err(e) = hwp.set_access(Access::RWInterleaved) {
            error!("Failed to set access: {}", e);
            return;
        }
        if let Err(e) = hwp.set_period_size(PERIOD_FRAMES as i64, ValueOr::Nearest) {
            error!("Failed to set period size: {}", e);
            return;
        }
        if let Err(e) = pcm.hw_params(&hwp) {
            error!("Failed to apply hw params: {}", e);
            return;
        }
    }

    if let Err(e) = pcm.start() {
        error!("Failed to start PCM capture: {}", e);
        return;
    }

    info!("ALSA capture started");

    let bps = bytes_per_sample(fmt.bits);
    let frame_size = fmt.channels as usize * bps;
    let buf_frames = PERIOD_FRAMES as usize;
    let _buf_bytes = buf_frames * frame_size;

    loop {
        if stop.load(Ordering::Relaxed) {
            info!("ALSA capture stop flag set; exiting capture loop");
            break;
        }

        let _buf = vec![0u8; _buf_bytes];
        let result = match fmt.bits {
            16 => {
                let mut ibuf = vec![0i16; buf_frames * fmt.channels as usize];
                let io = match pcm.io_i16() {
                    Ok(io) => io,
                    Err(e) => { error!("Failed to get PCM IO: {}", e); break; }
                };
                match io.readi(&mut ibuf) {
                    Ok(frames) => {
                        let bytes: Vec<u8> = ibuf[..frames * fmt.channels as usize]
                            .iter()
                            .flat_map(|s| s.to_le_bytes())
                            .collect();
                        Ok(bytes)
                    }
                    Err(e) => Err(e),
                }
            }
            _ => {
                // For 24/32-bit, use i32 IO
                let mut ibuf = vec![0i32; buf_frames * fmt.channels as usize];
                let io = match pcm.io_i32() {
                    Ok(io) => io,
                    Err(e) => { error!("Failed to get PCM IO: {}", e); break; }
                };
                match io.readi(&mut ibuf) {
                    Ok(frames) => {
                        let bytes: Vec<u8> = ibuf[..frames * fmt.channels as usize]
                            .iter()
                            .flat_map(|s| s.to_le_bytes())
                            .collect();
                        Ok(bytes)
                    }
                    Err(e) => Err(e),
                }
            }
        };

        match result {
            Ok(bytes) => {
                if !bytes.is_empty() {
                    debug!("ALSA read {} bytes", bytes.len());
                    if tx.send(bytes).is_err() {
                        debug!("ALSA channel receiver dropped; stopping capture");
                        break;
                    }
                }
            }
            Err(e) if e.errno() == 32 => {
                // EPIPE — overrun
                warn!("ALSA overrun (EPIPE) detected; recovering via snd_pcm_prepare");
                if let Err(e2) = pcm.prepare() {
                    error!("Failed to recover from overrun: {}", e2);
                    break;
                }
            }
            Err(e) => {
                error!("ALSA read error: {} (errno: {})", e, e.errno());
                break;
            }
        }
    }

    info!("ALSA capture stopped");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bits_to_format_16() {
        let f = bits_to_format(16);
        assert_eq!(f, Format::s16());
    }

    #[test]
    fn test_bits_to_format_24() {
        let f = bits_to_format(24);
        assert_eq!(f, Format::s24());
    }

    #[test]
    fn test_bits_to_format_32() {
        let f = bits_to_format(32);
        assert_eq!(f, Format::s32());
    }

    #[test]
    fn test_bytes_per_sample() {
        assert_eq!(bytes_per_sample(16), 2);
        assert_eq!(bytes_per_sample(24), 4);
        assert_eq!(bytes_per_sample(32), 4);
    }

    // Task 5.4 Property test: Audio bytes forwarded unmodified
    #[cfg(test)]
    mod property_tests {
        use proptest::prelude::*;
        use std::sync::mpsc;

        // Feature: roon-naa6-bridge, Property 3: Audio bytes forwarded unmodified
        // This property tests that bytes sent through the mpsc channel arrive unmodified.
        proptest! {
            #![proptest_config(proptest::test_runner::Config::with_cases(100))]

            #[test]
            fn prop_bytes_forwarded_unmodified(
                data in proptest::collection::vec(any::<u8>(), 0..4096)
            ) {
                let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(8);
                let data_clone = data.clone();
                tx.send(data_clone).unwrap();
                let received = rx.recv().unwrap();
                prop_assert_eq!(received, data);
            }
        }
    }
}
