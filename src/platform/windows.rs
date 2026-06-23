use crate::config::{Action, InputKey, RuntimeBindings};
use anyhow::{anyhow, Result};
use log::info;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, OnceLock};
use std::thread::{self, JoinHandle};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

static RUNTIME: OnceLock<Arc<RuntimeBindings>> = OnceLock::new();
static HELD_TARGETS: [AtomicU16; InputKey::COUNT] = [const { AtomicU16::new(0) }; InputKey::COUNT];
static CAPS_CAPTURED: AtomicBool = AtomicBool::new(false);
const RELEASE_ALREADY_SENT: u16 = u16::MAX;

pub struct KeyboardManager {
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

impl KeyboardManager {
    pub fn new(runtime: Arc<RuntimeBindings>) -> Result<Self> {
        RUNTIME
            .set(runtime)
            .map_err(|_| anyhow!("keyboard manager has already been initialized"))?;

        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("nocaps-windows-input".to_owned())
            .spawn(move || run_hook_thread(ready_tx))?;
        let thread_id = ready_rx
            .recv()
            .map_err(|_| anyhow!("Windows input thread stopped during startup"))?
            .map_err(|message| anyhow!(message))?;
        info!("Windows keyboard hook installed on dedicated high-priority thread");
        Ok(Self {
            thread_id,
            thread: Some(thread),
        })
    }
}

fn run_hook_thread(ready: mpsc::SyncSender<std::result::Result<u32, String>>) {
    let thread_id = unsafe { GetCurrentThreadId() };
    let mut message = MSG::default();
    // Explicitly create this thread's message queue before publishing its ID.
    let _ = unsafe { PeekMessageW(&mut message, None, WM_USER, WM_USER, PM_NOREMOVE) };

    if !unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST) }.as_bool() {
        log::warn!("Windows denied high priority for the keyboard thread");
    }

    let hook = match unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), None, 0) } {
        Ok(hook) => hook,
        Err(error) => {
            let _ = ready.send(Err(error.to_string()));
            return;
        }
    };
    force_caps_lock_off();
    if ready.send(Ok(thread_id)).is_err() {
        let _ = unsafe { UnhookWindowsHookEx(hook) };
        return;
    }

    while unsafe { GetMessageW(&mut message, None, 0, 0) }.as_bool() {
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }

    release_held_targets(false);
    let _ = unsafe { UnhookWindowsHookEx(hook) };
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        return call_next_hook(code, wparam, lparam);
    }

    let event = unsafe { *(lparam.0 as *const KBDLLHOOKSTRUCT) };
    if event.flags.0 & LLKHF_INJECTED.0 != 0 {
        return call_next_hook(code, wparam, lparam);
    }

    let virtual_key = event.vkCode as i32;
    let source = key_from_windows_scan(event.scanCode, event.flags.0 & LLKHF_EXTENDED.0 != 0);
    let is_down = wparam.0 == WM_KEYDOWN as usize || wparam.0 == WM_SYSKEYDOWN as usize;
    let is_up = wparam.0 == WM_KEYUP as usize || wparam.0 == WM_SYSKEYUP as usize;
    if !is_down && !is_up {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    if virtual_key == VK_CAPITAL.0 as i32 {
        if is_up && CAPS_CAPTURED.swap(false, Ordering::Relaxed) {
            release_held_targets(true);
            return LRESULT(1);
        }
        if is_down && runtime_enabled() {
            force_caps_lock_off();
            CAPS_CAPTURED.store(true, Ordering::Relaxed);
            return LRESULT(1);
        }
    }

    if is_up {
        if let Some(source) = source {
            let target = HELD_TARGETS[source.index()].swap(0, Ordering::Relaxed);
            if target == RELEASE_ALREADY_SENT {
                return LRESULT(1);
            }
            if target != 0 {
                send_key(target, false);
                return LRESULT(1);
            }
        }
    }

    if is_down && CAPS_CAPTURED.load(Ordering::Relaxed) {
        if let Some(source) = source {
            if let Some(target) = configured_target(source) {
                HELD_TARGETS[source.index()].store(target, Ordering::Relaxed);
                send_key(target, true);
                return LRESULT(1);
            }
        }
    }

    call_next_hook(code, wparam, lparam)
}

fn call_next_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn configured_target(source: InputKey) -> Option<u16> {
    RUNTIME.get()?.action_for(source).map(windows_code)
}

fn runtime_enabled() -> bool {
    RUNTIME.get().is_some_and(|runtime| runtime.is_enabled())
}

fn send_key(code: u16, down: bool) {
    let mut flags = if down {
        KEYBD_EVENT_FLAGS(0)
    } else {
        KEYEVENTF_KEYUP
    };
    if is_extended(code) {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(code),
                dwFlags: flags,
                ..Default::default()
            },
        },
    };
    unsafe {
        SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
    }
}

fn force_caps_lock_off() {
    if unsafe { GetKeyState(VK_CAPITAL.0 as i32) } & 1 != 0 {
        send_key(VK_CAPITAL.0, true);
        send_key(VK_CAPITAL.0, false);
    }
}

fn release_held_targets(swallow_physical_release: bool) {
    for target in &HELD_TARGETS {
        let held = target.swap(0, Ordering::Relaxed);
        if held != 0 && held != RELEASE_ALREADY_SENT {
            send_key(held, false);
            if swallow_physical_release {
                target.store(RELEASE_ALREADY_SENT, Ordering::Relaxed);
            }
        }
    }
}

fn is_extended(code: u16) -> bool {
    matches!(
        VIRTUAL_KEY(code),
        VK_UP
            | VK_DOWN
            | VK_LEFT
            | VK_RIGHT
            | VK_HOME
            | VK_END
            | VK_PRIOR
            | VK_NEXT
            | VK_INSERT
            | VK_DELETE
    )
}

fn key_from_windows_scan(scan: u32, extended: bool) -> Option<InputKey> {
    Some(match (scan, extended) {
        (0x1E, false) => InputKey::A,
        (0x30, false) => InputKey::B,
        (0x2E, false) => InputKey::C,
        (0x20, false) => InputKey::D,
        (0x12, false) => InputKey::E,
        (0x21, false) => InputKey::F,
        (0x22, false) => InputKey::G,
        (0x23, false) => InputKey::H,
        (0x17, false) => InputKey::I,
        (0x24, false) => InputKey::J,
        (0x25, false) => InputKey::K,
        (0x26, false) => InputKey::L,
        (0x32, false) => InputKey::M,
        (0x31, false) => InputKey::N,
        (0x18, false) => InputKey::O,
        (0x19, false) => InputKey::P,
        (0x10, false) => InputKey::Q,
        (0x13, false) => InputKey::R,
        (0x1F, false) => InputKey::S,
        (0x14, false) => InputKey::T,
        (0x16, false) => InputKey::U,
        (0x2F, false) => InputKey::V,
        (0x11, false) => InputKey::W,
        (0x2D, false) => InputKey::X,
        (0x15, false) => InputKey::Y,
        (0x2C, false) => InputKey::Z,
        (0x0B, false) | (0x52, false) => InputKey::Digit0,
        (0x02, false) | (0x4F, false) => InputKey::Digit1,
        (0x03, false) | (0x50, false) => InputKey::Digit2,
        (0x04, false) | (0x51, false) => InputKey::Digit3,
        (0x05, false) | (0x4B, false) => InputKey::Digit4,
        (0x06, false) | (0x4C, false) => InputKey::Digit5,
        (0x07, false) | (0x4D, false) => InputKey::Digit6,
        (0x08, false) | (0x47, false) => InputKey::Digit7,
        (0x09, false) | (0x48, false) => InputKey::Digit8,
        (0x0A, false) | (0x49, false) => InputKey::Digit9,
        (0x29, false) => InputKey::Backquote,
        (0x0C, false) | (0x4A, false) => InputKey::Minus,
        (0x0D, false) | (0x4E, false) => InputKey::Equal,
        (0x1A, false) => InputKey::LeftBracket,
        (0x1B, false) => InputKey::RightBracket,
        (0x2B, false) | (0x56, false) => InputKey::Backslash,
        (0x27, false) => InputKey::Semicolon,
        (0x28, false) => InputKey::Quote,
        (0x33, false) => InputKey::Comma,
        (0x34, false) | (0x53, false) => InputKey::Period,
        (0x35, false) | (0x35, true) => InputKey::Slash,
        (0x0F, false) => InputKey::Tab,
        (0x39, false) => InputKey::Space,
        (0x1C, false) | (0x1C, true) => InputKey::Enter,
        (0x01, false) => InputKey::Escape,
        (0x0E, false) => InputKey::Backspace,
        (0x53, true) => InputKey::Delete,
        (0x52, true) => InputKey::Insert,
        (0x47, true) => InputKey::Home,
        (0x4F, true) => InputKey::End,
        (0x49, true) => InputKey::PageUp,
        (0x51, true) => InputKey::PageDown,
        (0x48, true) => InputKey::ArrowUp,
        (0x50, true) => InputKey::ArrowDown,
        (0x4B, true) => InputKey::ArrowLeft,
        (0x4D, true) => InputKey::ArrowRight,
        (0x3B, false) => InputKey::F1,
        (0x3C, false) => InputKey::F2,
        (0x3D, false) => InputKey::F3,
        (0x3E, false) => InputKey::F4,
        (0x3F, false) => InputKey::F5,
        (0x40, false) => InputKey::F6,
        (0x41, false) => InputKey::F7,
        (0x42, false) => InputKey::F8,
        (0x43, false) => InputKey::F9,
        (0x44, false) => InputKey::F10,
        (0x57, false) => InputKey::F11,
        (0x58, false) => InputKey::F12,
        (0x64, false) => InputKey::F13,
        (0x65, false) => InputKey::F14,
        (0x66, false) => InputKey::F15,
        (0x67, false) => InputKey::F16,
        (0x68, false) => InputKey::F17,
        (0x69, false) => InputKey::F18,
        (0x6A, false) => InputKey::F19,
        (0x6B, false) => InputKey::F20,
        (0x6C, false) => InputKey::F21,
        (0x6D, false) => InputKey::F22,
        (0x6E, false) => InputKey::F23,
        (0x6F, false) => InputKey::F24,
        _ => return None,
    })
}

fn windows_code(action: Action) -> u16 {
    match action {
        Action::LeftControl => VK_LCONTROL.0,
        Action::LeftShift => VK_LSHIFT.0,
        Action::LeftAlt => VK_LMENU.0,
        Action::LeftMeta => VK_LWIN.0,
        Action::ArrowUp => VK_UP.0,
        Action::ArrowDown => VK_DOWN.0,
        Action::ArrowLeft => VK_LEFT.0,
        Action::ArrowRight => VK_RIGHT.0,
        Action::Home => VK_HOME.0,
        Action::End => VK_END.0,
        Action::PageUp => VK_PRIOR.0,
        Action::PageDown => VK_NEXT.0,
        Action::Backspace => VK_BACK.0,
        Action::Delete => VK_DELETE.0,
        Action::Enter => VK_RETURN.0,
        Action::Escape => VK_ESCAPE.0,
        Action::Tab => VK_TAB.0,
        Action::Space => VK_SPACE.0,
        Action::VolumeUp => VK_VOLUME_UP.0,
        Action::VolumeDown => VK_VOLUME_DOWN.0,
        Action::VolumeMute => VK_VOLUME_MUTE.0,
        Action::MediaPrevious => VK_MEDIA_PREV_TRACK.0,
        Action::MediaPlayPause => VK_MEDIA_PLAY_PAUSE.0,
        Action::MediaNext => VK_MEDIA_NEXT_TRACK.0,
    }
}

impl Drop for KeyboardManager {
    fn drop(&mut self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_codes_are_physical_and_cover_extended_keys() {
        assert_eq!(key_from_windows_scan(0x1E, false), Some(InputKey::A));
        assert_eq!(
            key_from_windows_scan(0x27, false),
            Some(InputKey::Semicolon)
        );
        assert_eq!(key_from_windows_scan(0x48, true), Some(InputKey::ArrowUp));
    }

    #[test]
    fn numpad_keys_share_their_configurable_character_binding() {
        assert_eq!(key_from_windows_scan(0x50, false), Some(InputKey::Digit2));
        assert_eq!(key_from_windows_scan(0x35, true), Some(InputKey::Slash));
    }
}
