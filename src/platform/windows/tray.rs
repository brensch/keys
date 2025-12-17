use anyhow::{anyhow, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use windows::core::PCWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub struct TrayManager {
    hwnd: HWND,
    hicon: HICON,
}

impl TrayManager {
    pub fn new(running: Arc<AtomicBool>) -> Result<Self> {
        unsafe {
            let class_name = wide_string("KeyboardRemapperClass");
            let hinstance = GetModuleHandleW(None)?;

            // Load the icon from the embedded resources
            let hicon = LoadIconW(hinstance, PCWSTR::from_raw(1 as *const u16))
                .map_err(|_| anyhow!("Failed to load icon from resource"))?;

            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(Self::window_proc),
                hInstance: hinstance,
                lpszClassName: PCWSTR(class_name.as_ptr()),
                hIcon: hicon,
                ..Default::default()
            };

            let atom = RegisterClassExW(&wc);
            if atom == 0 {
                DestroyIcon(hicon);
                return Err(anyhow!("Failed to register window class"));
            }

            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(class_name.as_ptr()),
                PCWSTR(wide_string("keys").as_ptr()),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                400,
                300,
                None,
                None,
                hinstance,
                Some(Box::into_raw(Box::new(TrayWindowData {
                    running: running.clone(),
                })) as *const _ as *const _),
            );

            if hwnd.0 == 0 {
                DestroyIcon(hicon);
                return Err(anyhow!("Failed to create window"));
            }

            ShowWindow(hwnd, SW_HIDE);

            let mut nid = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: hwnd,
                uID: 1,
                uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
                uCallbackMessage: WM_USER + 1,
                hIcon: hicon,
                ..Default::default()
            };

            let tooltip = wide_string("keys");
            nid.szTip[..tooltip.len()].copy_from_slice(&tooltip);

            if !Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
                DestroyWindow(hwnd);
                DestroyIcon(hicon);
                return Err(anyhow!("Failed to create tray icon"));
            }

            Ok(Self { hwnd, hicon })
        }
    }

    extern "system" fn window_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        unsafe {
            match msg {
                WM_CREATE => {
                    let create_struct = &*(lparam.0 as *const CREATESTRUCTW);
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, create_struct.lpCreateParams as isize);
                    LRESULT(0)
                }
                v if v == WM_USER + 1 => {
                    if lparam.0 as u32 == WM_RBUTTONUP {
                        let mut point = POINT::default();
                        GetCursorPos(&mut point);

                        let menu = CreatePopupMenu().unwrap();
                        InsertMenuW(
                            menu,
                            0,
                            MENU_ITEM_FLAGS(MF_STRING.0),
                            1,
                            PCWSTR::from_raw(wide_string("Exit").as_ptr()),
                        );

                        SetForegroundWindow(hwnd);
                        let cmd = TrackPopupMenu(
                            menu,
                            TPM_RETURNCMD | TPM_NONOTIFY,
                            point.x,
                            point.y,
                            0,
                            hwnd,
                            None,
                        );

                        if cmd.0 == 1 {
                            let data_ptr =
                                GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut TrayWindowData;
                            if !data_ptr.is_null() {
                                (*data_ptr).running.store(false, Ordering::SeqCst);
                            }
                            PostMessageW(hwnd, WM_DESTROY, WPARAM(0), LPARAM(0));
                        }

                        DestroyMenu(menu);
                        LRESULT(0)
                    } else {
                        DefWindowProcW(hwnd, msg, wparam, lparam)
                    }
                }
                WM_DESTROY => {
                    let data_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut TrayWindowData;
                    if !data_ptr.is_null() {
                        let _ = Box::from_raw(data_ptr);
                    }
                    PostQuitMessage(0);
                    LRESULT(0)
                }
                WM_CLOSE => {
                    ShowWindow(hwnd, SW_HIDE);
                    LRESULT(0)
                }
                _ => DefWindowProcW(hwnd, msg, wparam, lparam),
            }
        }
    }
}

impl Drop for TrayManager {
    fn drop(&mut self) {
        unsafe {
            let nid = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: self.hwnd,
                uID: 1,
                ..Default::default()
            };
            Shell_NotifyIconW(NIM_DELETE, &nid);
            DestroyIcon(self.hicon);
            DestroyWindow(self.hwnd);
        }
    }
}

struct TrayWindowData {
    running: Arc<AtomicBool>,
}

fn wide_string(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
