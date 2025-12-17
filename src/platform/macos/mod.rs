pub mod keyboard;
pub mod tray;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tao::event_loop::{ControlFlow, EventLoop};
use tray_icon::menu::MenuEvent;

pub fn run_event_loop(running: Arc<AtomicBool>) {
    let event_loop = EventLoop::new();
    
    event_loop.run(move |_event, _, control_flow| {
        *control_flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(50));

        if let Ok(_event) = MenuEvent::receiver().try_recv() {
             // Ideally we check if it's the quit item, but for now any menu item is quit
             running.store(false, Ordering::SeqCst);
             *control_flow = ControlFlow::Exit;
        }

        if !running.load(Ordering::SeqCst) {
            *control_flow = ControlFlow::Exit;
        }
    });
}
