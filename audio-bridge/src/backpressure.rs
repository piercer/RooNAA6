use log::{info, warn};

/// Controls ALSA capture pause/resume based on NAA6 TCP buffer state
pub struct BackpressureController {
    paused: bool,
}

impl BackpressureController {
    pub fn new() -> Self {
        BackpressureController { paused: false }
    }

    /// Called when the NAA6 TCP send buffer is full (EAGAIN/EWOULDBLOCK).
    /// Returns true if the state changed (was not already paused).
    pub fn on_buffer_full(&mut self) -> bool {
        if !self.paused {
            self.paused = true;
            warn!("Backpressure: NAA6 buffer full — pausing ALSA capture");
            true
        } else {
            false
        }
    }

    /// Called when the NAA6 TCP socket becomes writable again.
    /// Returns true if the state changed (was paused).
    pub fn on_socket_drain(&mut self) -> bool {
        if self.paused {
            self.paused = false;
            info!("Backpressure: NAA6 socket drained — resuming ALSA capture");
            true
        } else {
            false
        }
    }

    /// Returns true if ALSA capture is currently paused
    pub fn is_paused(&self) -> bool {
        self.paused
    }
}

impl Default for BackpressureController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_state_not_paused() {
        let bp = BackpressureController::new();
        assert!(!bp.is_paused());
    }

    #[test]
    fn test_buffer_full_sets_paused() {
        let mut bp = BackpressureController::new();
        let changed = bp.on_buffer_full();
        assert!(changed);
        assert!(bp.is_paused());
    }

    #[test]
    fn test_buffer_full_idempotent() {
        let mut bp = BackpressureController::new();
        bp.on_buffer_full();
        let changed = bp.on_buffer_full();
        assert!(!changed); // already paused, no state change
        assert!(bp.is_paused());
    }

    #[test]
    fn test_socket_drain_clears_paused() {
        let mut bp = BackpressureController::new();
        bp.on_buffer_full();
        let changed = bp.on_socket_drain();
        assert!(changed);
        assert!(!bp.is_paused());
    }

    #[test]
    fn test_socket_drain_when_not_paused_is_noop() {
        let mut bp = BackpressureController::new();
        let changed = bp.on_socket_drain();
        assert!(!changed);
        assert!(!bp.is_paused());
    }

    #[test]
    fn test_pause_resume_cycle() {
        let mut bp = BackpressureController::new();
        assert!(!bp.is_paused());
        bp.on_buffer_full();
        assert!(bp.is_paused());
        bp.on_socket_drain();
        assert!(!bp.is_paused());
        bp.on_buffer_full();
        assert!(bp.is_paused());
    }
}
