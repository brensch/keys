pub mod keyboard;
pub mod tray;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use windows::Win32::UI::WindowsAndMessaging::*;
use log::info;

pub fn run_event_loop(running: Arc<AtomicBool>) {
    let mut msg = MSG::default();
    unsafe {
        info!("Entering message loop");
        while running.load(Ordering::SeqCst) && GetMessageW(&mut msg, None, 0, 0).as_bool() {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
