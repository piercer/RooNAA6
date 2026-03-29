use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use log::{info, error};
use signal_hook::consts::{SIGTERM, SIGINT};
use signal_hook::flag;

/// Manages graceful shutdown on SIGTERM/SIGINT
pub struct ShutdownHandler {
    shutdown_flag: Arc<AtomicBool>,
}

impl ShutdownHandler {
    /// Create a new ShutdownHandler and register signal handlers
    pub fn new() -> Result<Self, std::io::Error> {
        let shutdown_flag = Arc::new(AtomicBool::new(false));
        flag::register(SIGTERM, Arc::clone(&shutdown_flag))?;
        flag::register(SIGINT, Arc::clone(&shutdown_flag))?;
        info!("ShutdownHandler registered for SIGTERM and SIGINT");
        Ok(ShutdownHandler { shutdown_flag })
    }

    /// Returns true if a shutdown signal has been received
    pub fn should_shutdown(&self) -> bool {
        self.shutdown_flag.load(Ordering::Relaxed)
    }

    /// Get a clone of the shutdown flag for use in other threads
    pub fn flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.shutdown_flag)
    }
}

impl Default for ShutdownHandler {
    fn default() -> Self {
        Self::new().expect("Failed to register signal handlers")
    }
}

/// Execute the graceful shutdown sequence with a 5-second hard-kill deadline.
/// Steps:
/// 1. Spawn a hard-kill thread that fires after 5 seconds (exits with code 1)
/// 2. Signal ALSA and session to stop
/// 3. Exit with code 0
pub fn run_shutdown_sequence(stop_flag: &Arc<AtomicBool>) {
    info!("Shutdown sequence started");

    // Spawn a hard-kill thread that fires after 5 seconds
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(5));
        error!("Shutdown deadline exceeded (5s); forcing exit with code 1");
        std::process::exit(1);
    });

    // Signal all threads to stop
    stop_flag.store(true, Ordering::Relaxed);
    info!("Stop flag set; waiting for threads to finish");

    // Give threads a moment to clean up
    std::thread::sleep(std::time::Duration::from_millis(500));

    info!("Shutdown sequence complete; exiting with code 0");
    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shutdown_flag_initially_false() {
        let handler = ShutdownHandler::new().unwrap();
        assert!(!handler.should_shutdown());
    }

    #[test]
    fn test_shutdown_flag_can_be_set_manually() {
        let handler = ShutdownHandler::new().unwrap();
        handler.flag().store(true, Ordering::Relaxed);
        assert!(handler.should_shutdown());
    }

    #[test]
    fn test_flag_clone_reflects_same_state() {
        let handler = ShutdownHandler::new().unwrap();
        let flag = handler.flag();
        flag.store(true, Ordering::Relaxed);
        assert!(handler.should_shutdown());
    }
}
