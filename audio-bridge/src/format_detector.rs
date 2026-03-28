use log::debug;
use crate::types::{FormatDescriptor, Encoding, DsdRate, VALID_PCM_SAMPLE_RATES, VALID_BIT_DEPTHS};

/// Detects format changes and builds FormatDescriptors
pub struct FormatDetector {
    last_format: Option<FormatDescriptor>,
}

impl FormatDetector {
    pub fn new() -> Self {
        FormatDetector { last_format: None }
    }

    /// Check if the given format differs from the last known format.
    /// Returns Some(new_format) if a change is detected, None otherwise.
    pub fn check_change(&mut self, new_format: FormatDescriptor) -> Option<FormatDescriptor> {
        let changed = match &self.last_format {
            None => true,
            Some(old) => old != &new_format,
        };

        if changed {
            debug!("FormatDetector: format change detected: {:?}", new_format);
            self.last_format = Some(new_format.clone());
            Some(new_format)
        } else {
            None
        }
    }

    /// Get the last known format
    pub fn last_format(&self) -> Option<&FormatDescriptor> {
        self.last_format.as_ref()
    }

    /// Validate a FormatDescriptor against the supported values
    pub fn validate(fmt: &FormatDescriptor) -> Result<(), String> {
        match fmt.encoding {
            Encoding::PCM => {
                if !VALID_PCM_SAMPLE_RATES.contains(&fmt.sample_rate) {
                    return Err(format!(
                        "Unsupported PCM sample rate: {}. Supported: {:?}",
                        fmt.sample_rate, VALID_PCM_SAMPLE_RATES
                    ));
                }
                if !VALID_BIT_DEPTHS.contains(&fmt.bit_depth) {
                    return Err(format!(
                        "Unsupported bit depth: {}. Supported: {:?}",
                        fmt.bit_depth, VALID_BIT_DEPTHS
                    ));
                }
                if fmt.channels < 1 || fmt.channels > 8 {
                    return Err(format!(
                        "Unsupported channel count: {}. Must be 1–8",
                        fmt.channels
                    ));
                }
            }
            Encoding::DSD_NATIVE | Encoding::DSD_DOP => {
                if fmt.dsd_rate.is_none() {
                    return Err("DSD format requires dsd_rate to be set".to_string());
                }
                if fmt.channels < 1 || fmt.channels > 8 {
                    return Err(format!(
                        "Unsupported channel count: {}. Must be 1–8",
                        fmt.channels
                    ));
                }
            }
        }
        Ok(())
    }
}

impl Default for FormatDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Encoding, FormatDescriptor, DsdRate};

    fn pcm_fmt(rate: u32, depth: u8, channels: u8) -> FormatDescriptor {
        FormatDescriptor {
            encoding: Encoding::PCM,
            sample_rate: rate,
            bit_depth: depth,
            channels,
            dsd_rate: None,
        }
    }

    #[test]
    fn test_first_format_always_triggers_change() {
        let mut detector = FormatDetector::new();
        let fmt = pcm_fmt(44100, 16, 2);
        let result = detector.check_change(fmt.clone());
        assert!(result.is_some());
        assert_eq!(result.unwrap(), fmt);
    }

    #[test]
    fn test_same_format_no_change() {
        let mut detector = FormatDetector::new();
        let fmt = pcm_fmt(44100, 16, 2);
        detector.check_change(fmt.clone());
        let result = detector.check_change(fmt);
        assert!(result.is_none());
    }

    #[test]
    fn test_different_sample_rate_triggers_change() {
        let mut detector = FormatDetector::new();
        detector.check_change(pcm_fmt(44100, 16, 2));
        let result = detector.check_change(pcm_fmt(96000, 16, 2));
        assert!(result.is_some());
        assert_eq!(result.unwrap().sample_rate, 96000);
    }

    #[test]
    fn test_validate_valid_pcm() {
        let fmt = pcm_fmt(44100, 24, 2);
        assert!(FormatDetector::validate(&fmt).is_ok());
    }

    #[test]
    fn test_validate_invalid_sample_rate() {
        let fmt = pcm_fmt(12345, 16, 2);
        assert!(FormatDetector::validate(&fmt).is_err());
    }

    #[test]
    fn test_validate_invalid_bit_depth() {
        let fmt = pcm_fmt(44100, 8, 2);
        assert!(FormatDetector::validate(&fmt).is_err());
    }

    #[test]
    fn test_validate_invalid_channels() {
        let fmt = pcm_fmt(44100, 16, 9);
        assert!(FormatDetector::validate(&fmt).is_err());
    }

    #[test]
    fn test_validate_dsd_without_rate_fails() {
        let fmt = FormatDescriptor {
            encoding: Encoding::DSD_NATIVE,
            sample_rate: 2822400,
            bit_depth: 1,
            channels: 2,
            dsd_rate: None,
        };
        assert!(FormatDetector::validate(&fmt).is_err());
    }

    #[test]
    fn test_validate_dsd_with_rate_ok() {
        let fmt = FormatDescriptor {
            encoding: Encoding::DSD_NATIVE,
            sample_rate: 2822400,
            bit_depth: 1,
            channels: 2,
            dsd_rate: Some(DsdRate::DSD64),
        };
        assert!(FormatDetector::validate(&fmt).is_ok());
    }
}
