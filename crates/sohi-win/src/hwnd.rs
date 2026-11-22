//! Window ownership and the shared window-procedure trampoline.
//!
//! Each Sohi window type implements [`WndHandler`]. [`create_window`] stores
//! the boxed handler in `GWLP_USERDATA`, the trampoline routes messages to
//! it, and `WM_NCDESTROY` frees it. [`OwnedHwnd`] destroys the window on drop.

use std::ffi::c_void;

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow,
    GWLP_USERDATA, GetWindowLongPtrW, HCURSOR, IDC_ARROW, LoadCursorW, RegisterClassExW,
    SetWindowLongPtrW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_NCCREATE, WM_NCDESTROY, WNDCLASSEXW,
};
use windows::core::{HSTRING, PCWSTR};

use crate::WinError;

/// Per-window message handling.
pub(crate) trait WndHandler {
    /// Handles `msg`. Returning `None` falls through to `DefWindowProcW`.
    fn on_message(&self, hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT>;
}

/// A window destroyed on drop. Only usable on the thread that created it.
#[derive(Debug)]
pub(crate) struct OwnedHwnd(HWND);

impl OwnedHwnd {
    /// The raw handle; valid while `self` lives.
    pub(crate) fn raw(&self) -> HWND {
        self.0
    }
}

impl Drop for OwnedHwnd {
    fn drop(&mut self) {
        // SAFETY: we created the window on this thread (OwnedHwnd is !Send
        // because HWND is a raw pointer). Destroying a window that is
        // already gone fails harmlessly.
        let _ = unsafe { DestroyWindow(self.0) };
    }
}

/// Splits a packed coordinate pair (`MAKELPARAM` layout: low word x, high
/// word y, each a signed 16-bit value) into screen or client coordinates.
pub(crate) fn signed_words(v: isize) -> (i32, i32) {
    let raw = v.to_le_bytes();
    (
        i32::from(i16::from_le_bytes([raw[0], raw[1]])),
        i32::from(i16::from_le_bytes([raw[2], raw[3]])),
    )
}

/// Window class and style parameters for [`create_window`].
pub(crate) struct WindowSpec<'a> {
    pub class_name: &'a str,
    pub ex_style: WINDOW_EX_STYLE,
    pub style: WINDOW_STYLE,
}

pub(crate) fn module_instance() -> Result<HINSTANCE, WinError> {
    // SAFETY: a null module name returns the handle of the current exe.
    let module =
        unsafe { GetModuleHandleW(None) }.map_err(|e| WinError::api("GetModuleHandleW", &e))?;
    Ok(HINSTANCE(module.0))
}

/// Registers the class (once per name) and creates a hidden window.
pub(crate) fn create_window(
    spec: &WindowSpec<'_>,
    handler: Box<dyn WndHandler>,
) -> Result<OwnedHwnd, WinError> {
    let instance = module_instance()?;
    let class = HSTRING::from(spec.class_name);
    // SAFETY: loading a stock system cursor has no preconditions.
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or(HCURSOR::default());
    let wc = WNDCLASSEXW {
        cbSize: u32::try_from(size_of::<WNDCLASSEXW>()).unwrap_or(u32::MAX),
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(trampoline),
        hInstance: instance,
        hCursor: cursor,
        lpszClassName: PCWSTR(class.as_ptr()),
        ..Default::default()
    };
    // SAFETY: `wc` is fully initialized and `class` outlives the call.
    // A zero return for an already-registered class is fine: CreateWindowExW
    // below reports any real problem.
    let _ = unsafe { RegisterClassExW(&raw const wc) };

    // Double box: GWLP_USERDATA holds one thin pointer.
    let state: *mut Box<dyn WndHandler> = Box::into_raw(Box::new(handler));
    // SAFETY: `state` is a valid, uniquely owned pointer. The trampoline
    // stores it on WM_NCCREATE and frees it on WM_NCDESTROY.
    let created = unsafe {
        CreateWindowExW(
            spec.ex_style,
            &class,
            &class,
            spec.style,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance),
            Some(state.cast::<c_void>().cast_const()),
        )
    };
    // On failure the handler is leaked: if WM_NCCREATE never ran nobody owns
    // it, and if it did WM_NCDESTROY already freed it. Not worth telling
    // apart on this startup-fatal path.
    let hwnd = created.map_err(|e| WinError::api("CreateWindowExW", &e))?;
    Ok(OwnedHwnd(hwnd))
}

unsafe extern "system" fn trampoline(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_NCCREATE {
        // SAFETY: for WM_NCCREATE, lparam points to the CREATESTRUCTW whose
        // lpCreateParams is the handler pointer from `create_window`.
        unsafe {
            let cs = &*(lparam.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
        }
    }
    // SAFETY: GWLP_USERDATA is either 0 or the pointer stored above.
    let state = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Box<dyn WndHandler>;
    if state.is_null() {
        // SAFETY: forwarding unmodified arguments.
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    if msg == WM_NCDESTROY {
        // SAFETY: last message for this window. Clear the slot first so no
        // later lookup sees a dangling pointer, then free the handler once.
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Box::from_raw(state));
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
    }
    // SAFETY: the pointer stays valid until WM_NCDESTROY, and the window is
    // only used on its creating thread. Handlers take `&self`, so a nested
    // (re-entrant) message only creates another shared reference.
    let handler = unsafe { &*state };
    match handler.on_message(hwnd, msg, wparam, lparam) {
        Some(result) => result,
        // SAFETY: forwarding unmodified arguments.
        None => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
