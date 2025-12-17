// #![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use anyhow::Result;
use log::info;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

mod platform;

use platform::{KeyboardManager, TrayManager, run_event_loop};

fn main() -> Result<()> {
    std::env::set_var("RUST_LOG", "debug");
    env_logger::init();
    info!("Starting keyboard remapper");

    // Create shared state for graceful shutdown
    let running = Arc::new(AtomicBool::new(true));

    // Initialize system tray
    let _tray = TrayManager::new(running.clone())?;

    // Initialize keyboard manager
    let _keyboard = KeyboardManager::new()?;
    info!("Keyboard hook installed");

    // Message loop
    run_event_loop(running);

    Ok(())
}
