use crate::config::RuntimeBindings;
use anyhow::Result;
use std::sync::Arc;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
pub use linux::KeyboardManager;
#[cfg(target_os = "macos")]
pub use macos::KeyboardManager;
#[cfg(target_os = "windows")]
pub use windows::KeyboardManager;

pub fn start_keyboard(runtime: Arc<RuntimeBindings>) -> Result<KeyboardManager> {
    KeyboardManager::new(runtime)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn elevate_input_thread() {
    use thread_priority::unix::{
        thread_native_id, RealtimeThreadSchedulePolicy, ThreadSchedulePolicy,
    };
    use thread_priority::ThreadPriority;

    let realtime = ThreadSchedulePolicy::Realtime(RealtimeThreadSchedulePolicy::RoundRobin);
    if let Err(error) = thread_priority::unix::set_thread_priority_and_policy(
        thread_native_id(),
        ThreadPriority::Max,
        realtime,
    ) {
        #[cfg(target_os = "linux")]
        log::warn!("realtime input priority denied ({error}); grant CAP_SYS_NICE to enable it");

        #[cfg(target_os = "macos")]
        if let Err(fallback_error) = ThreadPriority::Max.set_for_current() {
            log::warn!(
                "could not elevate input thread priority ({error}); fallback failed ({fallback_error})"
            );
        }
    }
}
