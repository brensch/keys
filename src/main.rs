#![windows_subsystem = "windows"]

use anyhow::Result;
use log::info;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use windows::Win32::UI::WindowsAndMessaging::*;

mod keyboard;
mod tray;

use keyboard::KeyboardManager;
use tray::TrayManager;

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
    let mut msg = MSG::default();
    unsafe {
        info!("Entering message loop");
        while running.load(Ordering::SeqCst) && GetMessageW(&mut msg, None, 0, 0).as_bool() {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    Ok(())
}
