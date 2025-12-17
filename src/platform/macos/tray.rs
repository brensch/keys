use anyhow::Result;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

pub struct TrayManager;

impl TrayManager {
    pub fn new(_running: Arc<AtomicBool>) -> Result<Self> {
        // TODO: Implement native macOS tray
        Ok(Self)
    }
}

pub fn run_event_loop(running: Arc<AtomicBool>) {
    // TODO: Implement native macOS event loop
    while running.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(100));
    }
}
