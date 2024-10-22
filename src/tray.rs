use anyhow::{anyhow, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use windows::core::PCWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
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

            // Create a simple 16x16 icon for the tray
            let bitmap_info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: 16,
                    biHeight: 16,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    biSizeImage: 0,
                    biXPelsPerMeter: 0,
                    biYPelsPerMeter: 0,
                    biClrUsed: 0,
                    biClrImportant: 0,
                },
                bmiColors: [RGBQUAD::default()],
            };

            let mut bits = vec![0u8; 16 * 16 * 4];
            let dc = GetDC(HWND(0));
            let color_dc = CreateCompatibleDC(dc);
            let bitmap = CreateDIBSection(
                color_dc,
                &bitmap_info,
                DIB_RGB_COLORS,
                &mut bits.as_mut_ptr() as *mut _ as *mut *mut std::ffi::c_void,
                None,
                0,
            )
            .unwrap();

            let mask_dc = CreateCompatibleDC(dc);
            let mask = CreateBitmap(16, 16, 1, 1, None);

            let old_color = SelectObject(color_dc, bitmap);
            let old_mask = SelectObject(mask_dc, mask);

            // Fill the entire icon with a solid color (black in this case)
            let brush = CreateSolidBrush(COLORREF(0x00000000));
            FillRect(
                color_dc,
                &RECT {
                    left: 0,
                    top: 0,
                    right: 16,
                    bottom: 16,
                },
                brush,
            );
            DeleteObject(brush);

            // Create the icon
            let icon_info = ICONINFO {
                fIcon: BOOL::from(true),
                xHotspot: 0,
                yHotspot: 0,
                hbmMask: HBITMAP(mask.0),
                hbmColor: bitmap,
            };

            let hicon = CreateIconIndirect(&icon_info);

            // Clean up
            SelectObject(color_dc, old_color);
            SelectObject(mask_dc, old_mask);
            DeleteDC(color_dc);
            DeleteDC(mask_dc);
            DeleteObject(bitmap);
            DeleteObject(mask);
            ReleaseDC(HWND(0), dc);

            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(Self::window_proc),
                hInstance: hinstance,
                lpszClassName: PCWSTR(class_name.as_ptr()),
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
                PCWSTR(wide_string("Keyboard Remapper").as_ptr()),
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

            let tooltip = wide_string("Keyboard Remapper");
            nid.szTip[..tooltip.len()].copy_from_slice(&tooltip);

            if !Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
                DestroyWindow(hwnd);
                DestroyIcon(hicon);
                return Err(anyhow!("Failed to create tray icon"));
            }

            Ok(Self { hwnd, hicon })
        }
    }

    // ... rest of the implementation remains the same
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
