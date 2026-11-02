//! Ventana de Windows que aloja la interfaz nativa de un plugin.
//!
//! Se crea en el hilo principal, cuyo bucle de mensajes (el de la aplicación)
//! también despacha los de estas ventanas.

use std::ptr;
use std::sync::{Arc, Once, Weak};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{GetSysColorBrush, COLOR_WINDOW};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowLongPtrW, GetWindowLongW, IsWindow,
    LoadCursorW, RegisterClassExW, SetForegroundWindow, SetWindowLongPtrW, SetWindowLongW, SetWindowPos, ShowWindow,
    CW_USEDEFAULT, GWLP_USERDATA, GWL_EXSTYLE, GWL_STYLE, IDC_ARROW, SIZE_MINIMIZED, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOZORDER, SW_SHOW, WM_CLOSE, WM_NCDESTROY, WM_SIZE, WNDCLASSEXW, WS_CAPTION, WS_CLIPCHILDREN, WS_EX_TOOLWINDOW,
    WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_OVERLAPPED, WS_SYSMENU, WS_THICKFRAME,
};

use crate::{notify_editor_closed, owner_window, Instance};

const CLASS: &str = "MusicNovaPluginWindow\0";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Ventana abierta; al soltarse se destruye.
pub(crate) struct Editor {
    hwnd: isize,
}

impl Drop for Editor {
    fn drop(&mut self) {
        unsafe {
            let hwnd = self.hwnd as HWND;
            if IsWindow(hwnd) != 0 {
                DestroyWindow(hwnd);
            }
        }
    }
}

struct WindowData {
    inst: Weak<Instance>,
    resizable: bool,
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let data = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WindowData;
    match msg {
        WM_CLOSE => {
            match data.as_ref().and_then(|d| d.inst.upgrade()) {
                Some(inst) => {
                    inst.close_editor();
                    notify_editor_closed(inst.slot_id.as_deref());
                }
                None => {
                    DestroyWindow(hwnd);
                }
            }
            0
        }
        WM_SIZE => {
            if let Some(d) = data.as_ref() {
                if d.resizable && wparam as u32 != SIZE_MINIMIZED {
                    if let Some(inst) = d.inst.upgrade() {
                        let (w, h) = ((lparam & 0xFFFF) as u32, ((lparam >> 16) & 0xFFFF) as u32);
                        if let Some(b) = inst.backend.as_ref() {
                            b.gui_set_size(w, h);
                        }
                    }
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_NCDESTROY => {
            if !data.is_null() {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(data));
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn register_class() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        let class = wide(CLASS.trim_end_matches('\0'));
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: GetModuleHandleW(ptr::null()),
            hIcon: ptr::null_mut(),
            hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
            hbrBackground: GetSysColorBrush(COLOR_WINDOW),
            lpszMenuName: ptr::null(),
            lpszClassName: class.as_ptr(),
            hIconSm: ptr::null_mut(),
        };
        RegisterClassExW(&wc);
    });
}

/// Ajusta la ventana para que su área cliente mida `w`×`h`.
pub(crate) fn resize_client(hwnd: isize, w: u32, h: u32) {
    unsafe {
        let hwnd = hwnd as HWND;
        let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
        let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        let mut r = RECT { left: 0, top: 0, right: w as i32, bottom: h as i32 };
        AdjustWindowRectEx(&mut r, style, 0, ex);
        SetWindowPos(hwnd, ptr::null_mut(), 0, 0, r.right - r.left, r.bottom - r.top, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

/// Hilo principal: abre la interfaz del plugin (o la trae al frente).
pub(crate) fn open(inst: &Arc<Instance>) -> Result<(), String> {
    if let Some(e) = inst.editor.lock().unwrap().as_ref() {
        unsafe { SetForegroundWindow(e.hwnd as HWND) };
        return Ok(());
    }
    let backend = inst.backend();
    register_class();
    let title = wide(&format!("{} · MusicNova", inst.plugin.name));
    let class = wide(CLASS.trim_end_matches('\0'));
    let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN;
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class.as_ptr(),
            title.as_ptr(),
            style,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            400,
            300,
            owner_window() as HWND,
            ptr::null_mut(),
            GetModuleHandleW(ptr::null()),
            ptr::null(),
        )
    };
    if hwnd.is_null() {
        return Err("no se pudo crear la ventana del plugin".into());
    }
    let editor = Editor { hwnd: hwnd as isize };
    let (w, h) = backend.gui_open(hwnd as isize)?; // si falla, `editor` destruye la ventana
    let resizable = backend.gui_can_resize();
    unsafe {
        if resizable {
            SetWindowLongW(hwnd, GWL_STYLE, (style | WS_THICKFRAME | WS_MAXIMIZEBOX) as i32);
        }
        let data = Box::into_raw(Box::new(WindowData { inst: Arc::downgrade(inst), resizable }));
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, data as isize);
        resize_client(hwnd as isize, w, h);
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
    }
    *inst.editor.lock().unwrap() = Some(editor);
    Ok(())
}
