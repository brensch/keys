#[cfg(target_os = "windows")]
pub mod windows;
#[cfg(target_os = "windows")]
pub use windows::keyboard::KeyboardManager;
#[cfg(target_os = "windows")]
pub use windows::tray::TrayManager;
#[cfg(target_os = "windows")]
pub use windows::run_event_loop;

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "macos")]
pub use macos::keyboard::KeyboardManager;
#[cfg(target_os = "macos")]
pub use macos::tray::TrayManager;
#[cfg(target_os = "macos")]
pub use macos::run_event_loop;
