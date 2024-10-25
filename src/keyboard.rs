use anyhow::Result;
use log::info;
use std::sync::atomic::{AtomicBool, Ordering};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

// Static mutable variables to track key states
static mut CAPS_LOCK_HELD: bool = false;
static mut CTRL_DOWN: bool = false;
static mut SHIFT_DOWN: bool = false;

// AtomicBool for logging control
pub static LOGGING_ENABLED: AtomicBool = AtomicBool::new(false);

pub struct KeyboardManager {
    hook: HHOOK,
}

impl KeyboardManager {
    pub fn new() -> Result<Self> {
        unsafe {
            let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(Self::keyboard_hook_proc), None, 0)?;

            info!("Keyboard hook installed successfully");
            Ok(Self { hook })
        }
    }

    unsafe extern "system" fn keyboard_hook_proc(
        code: i32,
        w_param: WPARAM,
        l_param: LPARAM,
    ) -> LRESULT {
        if code < 0 {
            return CallNextHookEx(None, code, w_param, l_param);
        }

        let kbd_struct = *(l_param.0 as *const KBDLLHOOKSTRUCT);
        let vk_code = kbd_struct.vkCode as i32;
        let flags = kbd_struct.flags;
        let is_injected = (flags.0 & LLKHF_INJECTED.0) != 0;

        if LOGGING_ENABLED.load(Ordering::SeqCst) {
            log::debug!("Processing key: {:#x}", vk_code);
        }

        if is_injected {
            return CallNextHookEx(None, code, w_param, l_param);
        }

        let is_key_down = w_param.0 == WM_KEYDOWN as usize || w_param.0 == WM_SYSKEYDOWN as usize;
        let is_key_up = w_param.0 == WM_KEYUP as usize || w_param.0 == WM_SYSKEYUP as usize;

        // Update CapsLock state
        if vk_code == VK_CAPITAL.0 as i32 {
            if LOGGING_ENABLED.load(Ordering::SeqCst) {
                log::debug!("Processing CapsLock key");
            }
            if is_key_down {
                CAPS_LOCK_HELD = true;
            } else if is_key_up {
                CAPS_LOCK_HELD = false;

                // Only release Ctrl/Shift if they were activated through our remapping
                if CTRL_DOWN {
                    let mut input = INPUT::default();
                    input.r#type = INPUT_KEYBOARD;
                    input.Anonymous.ki.wVk = VK_CONTROL;
                    input.Anonymous.ki.dwFlags = KEYEVENTF_KEYUP;
                    SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
                    CTRL_DOWN = false;
                }
                if SHIFT_DOWN {
                    let mut input = INPUT::default();
                    input.r#type = INPUT_KEYBOARD;
                    input.Anonymous.ki.wVk = VK_SHIFT;
                    input.Anonymous.ki.dwFlags = KEYEVENTF_KEYUP;
                    SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
                    SHIFT_DOWN = false;
                }
            }
            return LRESULT(1);
        }

        // Let Alt key events pass through
        if vk_code == VK_MENU.0 as i32
            || vk_code == VK_LMENU.0 as i32
            || vk_code == VK_RMENU.0 as i32
        {
            return CallNextHookEx(None, code, w_param, l_param);
        }

        if CAPS_LOCK_HELD {
            if LOGGING_ENABLED.load(Ordering::SeqCst) {
                log::debug!("CapsLock is held, checking key: {:#x}", vk_code);
            }
            let mut input = INPUT::default();
            input.r#type = INPUT_KEYBOARD;

            match vk_code {
                0x41 => {
                    // 'A' key - Control
                    input.Anonymous.ki.wVk = VK_CONTROL;
                    if is_key_down {
                        CTRL_DOWN = true;
                        input.Anonymous.ki.dwFlags = KEYBD_EVENT_FLAGS(0);
                    } else {
                        CTRL_DOWN = false;
                        input.Anonymous.ki.dwFlags = KEYEVENTF_KEYUP;
                    }
                    SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
                    return LRESULT(1);
                }
                0x53 => {
                    // 'S' key - Shift
                    input.Anonymous.ki.wVk = VK_SHIFT;
                    if is_key_down {
                        SHIFT_DOWN = true;
                        input.Anonymous.ki.dwFlags = KEYBD_EVENT_FLAGS(0);
                    } else {
                        SHIFT_DOWN = false;
                        input.Anonymous.ki.dwFlags = KEYEVENTF_KEYUP;
                    }
                    SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
                    return LRESULT(1);
                }
                0x49 => {
                    // 'I' key - Up Arrow
                    input.Anonymous.ki.wVk = VK_UP;
                    input.Anonymous.ki.dwFlags = if is_key_down {
                        KEYBD_EVENT_FLAGS(0)
                    } else {
                        KEYEVENTF_KEYUP
                    };
                    SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
                    return LRESULT(1);
                }
                0x4B => {
                    // 'K' key - Down Arrow
                    input.Anonymous.ki.wVk = VK_DOWN;
                    input.Anonymous.ki.dwFlags = if is_key_down {
                        KEYBD_EVENT_FLAGS(0)
                    } else {
                        KEYEVENTF_KEYUP
                    };
                    SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
                    return LRESULT(1);
                }
                0x4A => {
                    // 'J' key - Left Arrow
                    input.Anonymous.ki.wVk = VK_LEFT;
                    input.Anonymous.ki.dwFlags = if is_key_down {
                        KEYBD_EVENT_FLAGS(0)
                    } else {
                        KEYEVENTF_KEYUP
                    };
                    SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
                    return LRESULT(1);
                }
                0x4C => {
                    // 'L' key - Right Arrow
                    input.Anonymous.ki.wVk = VK_RIGHT;
                    input.Anonymous.ki.dwFlags = if is_key_down {
                        KEYBD_EVENT_FLAGS(0)
                    } else {
                        KEYEVENTF_KEYUP
                    };
                    SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
                    return LRESULT(1);
                }
                0x48 => {
                    // 'H' key - Home
                    input.Anonymous.ki.wVk = VK_HOME;
                    input.Anonymous.ki.dwFlags = if is_key_down {
                        KEYBD_EVENT_FLAGS(0)
                    } else {
                        KEYEVENTF_KEYUP
                    };
                    SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
                    return LRESULT(1);
                }
                0xBA => {
                    // Semicolon - End
                    input.Anonymous.ki.wVk = VK_END;
                    input.Anonymous.ki.dwFlags = if is_key_down {
                        KEYBD_EVENT_FLAGS(0)
                    } else {
                        KEYEVENTF_KEYUP
                    };
                    SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
                    return LRESULT(1);
                }
                0x57 => {
                    // 'W' key - Volume Up
                    if is_key_down {
                        keybd_event(VK_VOLUME_UP.0 as u8, 0, KEYBD_EVENT_FLAGS(0), 0);
                    }
                    return LRESULT(1);
                }
                0x51 => {
                    // 'Q' key - Volume Down
                    if is_key_down {
                        keybd_event(VK_VOLUME_DOWN.0 as u8, 0, KEYBD_EVENT_FLAGS(0), 0);
                    }
                    return LRESULT(1);
                }
                0x09 => {
                    // Tab - Volume Mute
                    if is_key_down {
                        keybd_event(VK_VOLUME_MUTE.0 as u8, 0, KEYBD_EVENT_FLAGS(0), 0);
                    }
                    return LRESULT(1);
                }
                0x45 => {
                    // 'E' key - Media Previous
                    if is_key_down {
                        keybd_event(VK_MEDIA_PREV_TRACK.0 as u8, 0, KEYBD_EVENT_FLAGS(0), 0);
                    }
                    return LRESULT(1);
                }
                0x52 => {
                    // 'R' key - Media Play/Pause
                    if is_key_down {
                        keybd_event(VK_MEDIA_PLAY_PAUSE.0 as u8, 0, KEYBD_EVENT_FLAGS(0), 0);
                    }
                    return LRESULT(1);
                }
                0x54 => {
                    // 'T' key - Media Next
                    if is_key_down {
                        keybd_event(VK_MEDIA_NEXT_TRACK.0 as u8, 0, KEYBD_EVENT_FLAGS(0), 0);
                    }
                    return LRESULT(1);
                }
                0x4F => {
                    // 'O' key - Delete
                    input.Anonymous.ki.wVk = VK_DELETE;
                    input.Anonymous.ki.dwFlags = if is_key_down {
                        KEYBD_EVENT_FLAGS(0)
                    } else {
                        KEYEVENTF_KEYUP
                    };
                    SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
                    return LRESULT(1);
                }
                0x55 => {
                    // 'U' key - Backspace
                    input.Anonymous.ki.wVk = VK_BACK;
                    input.Anonymous.ki.dwFlags = if is_key_down {
                        KEYBD_EVENT_FLAGS(0)
                    } else {
                        KEYEVENTF_KEYUP
                    };
                    SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
                    return LRESULT(1);
                }
                _ => {
                    // For unhandled keys when CapsLock is held, pass them through
                    return CallNextHookEx(None, code, w_param, l_param);
                }
            }
        }

        CallNextHookEx(None, code, w_param, l_param)
    }
}

impl Drop for KeyboardManager {
    fn drop(&mut self) {
        unsafe {
            UnhookWindowsHookEx(self.hook);
        }
    }
}
