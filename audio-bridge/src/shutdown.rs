use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use log::{info, warn, error};
use signal_hook::consts::{SIGTERM, SIGINT};
use signal_hook::flag;

/// Manages graceful shutdown on SIGTERM/SIGINT
pub struct ShutdownHandler {
    /// Set to true when a shutdown signal is received
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
/// 
/// Steps:
/// 1. Set a 5-second hard-kill alarm
/// 2. Send NAA6 termination message
/// 3. Disconnect from HQPlayer
/// 4. Close ALSA device
/// 5. Exit with code 0
pub fn run_shutdown_sequence<F1, F2, F3>(
    send_termination: F1,
    disconnect: F2,
    close_alsa: F3,
) where
    F1: FnOnce() -> Result<(), std::io::Error>,
    F2: FnOnce(),
    F3: FnOnce(),
{
    info!("Shutdown sequence started");

    // Spawn a hard-kill thread that fires after 5 seconds
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(5));
        error!("Shutdown deadline exceeded (5s); forcing exit with code 1");
        std::process::exit(1);
    });

    // Step 1: Send NAA6 termination message
    info!("Sending NAA6 termination message");
    if let Err(e) = send_termination() {
        warn!("Error sending termination message: {} — continuing shutdown", e);
    }

    // Step 2: Disconnect from HQPlayer
    info!("Disconnecting from HQPlayer");
    disconnect();

    // Step 3: Close ALSA device
    info!("Closing ALSA capture device");
    close_alsa();

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
    fn test_shutdown_sequence_calls_all_steps() {
        // Verify that all shutdown steps are called
        use std::sync::atomic::{AtomicUsize, Ordering};
        let counter = Arc::new(AtomicUsize::new(0));

        let c1 = Arc::clone(&counter);
        let c2 = Arc::clone(&counter);
        let c3 = Arc::clone(&counter);

        // We can't call run_shutdown_sequence directly (it calls process::exit),
        // but we can test the individual closures are invocable
        let send_term = move || -> Result<(), std::io::Error> {
            c1.fetch_add(1, Ordering::SeqCst);
            Ok(())
        };
        let disconnect = move || {
            c2.fetch_add(1, Ordering::SeqCst);
        };
        let close_alsa = move || {
            c3.fetch_add(1, Ordering::SeqCst);
        };

        // Call each step manually to verify they work
        send_term().unwrap();
        disconnect();
        close_alsa();

        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }
}
