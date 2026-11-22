//! Hidden application window and the blocking message loop.
//!
//! P0 uses a plain `GetMessageW` loop, which blocks in the kernel while idle.
//! P1 replaces it with `MsgWaitForMultipleObjectsEx` for frame pacing.

use std::cell::RefCell;
use std::ffi::c_void;

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA,
    GetMessageW, GetWindowLongPtrW, MSG, PostMessageW, PostQuitMessage, RegisterClassExW,
    SetWindowLongPtrW, TranslateMessage, WM_CLOSE, WM_DESTROY, WM_NCCREATE, WM_NCDESTROY,
    WNDCLASSEXW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::{HSTRING, PCWSTR};

use crate::WinError;

/// Events delivered to the application from the window procedure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WinEvent {
    /// Another launch asked this instance to wake.
    WakeRequested,
}

type Handler = RefCell<Box<dyn FnMut(WinEvent)>>;

/// The hidden top-level window that receives app-wide messages.
///
/// It is never shown. Destroying it ends the message loop.
pub struct AppWindow {
    hwnd: HWND,
}

impl AppWindow {
    /// Registers `class_name` and creates the hidden window.
    ///
    /// `wake_message` is the registered message that maps to
    /// [`WinEvent::WakeRequested`]. `handler` runs on this thread for each event.
    ///
    /// # Errors
    /// Returns [`WinError`] if class registration or window creation fails.
    pub fn create(
        class_name: &str,
        wake_message: u32,
        handler: impl FnMut(WinEvent) + 'static,
    ) -> Result<Self, WinError> {
        // SAFETY: a null module name returns the handle of the current exe.
        let module =
            unsafe { GetModuleHandleW(None) }.map_err(|e| WinError::api("GetModuleHandleW", &e))?;
        let instance = HINSTANCE(module.0);
        let class = HSTRING::from(class_name);
        let wc = WNDCLASSEXW {
            cbSize: u32::try_from(size_of::<WNDCLASSEXW>()).unwrap_or(u32::MAX),
            lpfnWndProc: Some(wnd_proc),
            hInstance: instance,
            lpszClassName: PCWSTR(class.as_ptr()),
            ..Default::default()
        };
        // SAFETY: `wc` is fully initialized and `class` outlives the call.
        if unsafe { RegisterClassExW(&raw const wc) } == 0 {
            return Err(WinError::last("RegisterClassExW"));
        }

        let state = Box::new(WindowState {
            wake_message,
            handler: RefCell::new(Box::new(handler)),
        });
        let state_ptr = Box::into_raw(state);
        // SAFETY: the class was registered above. `state_ptr` is a valid,
        // uniquely owned pointer; the window procedure takes it in
        // WM_NCCREATE and frees it in WM_NCDESTROY.
        let created = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                &class,
                &class,
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance),
                Some(state_ptr.cast::<c_void>().cast_const()),
            )
        };
        // On failure the state is leaked: if WM_NCCREATE never ran, nobody
        // owns it, and if it did, WM_NCDESTROY already freed it. Telling the
        // two apart is not worth it on this startup-fatal path.
        let hwnd = created.map_err(|e| WinError::api("CreateWindowExW", &e))?;
        Ok(Self { hwnd })
    }

    /// Returns a `Send` reference for posting to this window from other threads.
    #[must_use]
    pub fn remote(&self) -> RemoteWindow {
        RemoteWindow(self.hwnd.0 as usize)
    }
}

impl Drop for AppWindow {
    fn drop(&mut self) {
        // SAFETY: `hwnd` was created on this thread; destroying an already
        // destroyed window fails harmlessly.
        let _ = unsafe { DestroyWindow(self.hwnd) };
    }
}

/// A thread-safe reference to an [`AppWindow`], usable only for posting.
#[derive(Debug, Clone, Copy)]
pub struct RemoteWindow(usize);

impl RemoteWindow {
    /// Asks the window to close, which ends the message loop.
    ///
    /// # Errors
    /// Returns [`WinError`] if the window no longer exists.
    pub fn post_close(self) -> Result<(), WinError> {
        let hwnd = HWND(self.0 as *mut c_void);
        // SAFETY: PostMessageW validates the window handle itself, and
        // WM_CLOSE carries no pointers.
        unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) }
            .map_err(|e| WinError::api("PostMessageW", &e))
    }
}

/// Runs the message loop on the current thread until `WM_QUIT`.
///
/// Returns the exit code passed to `PostQuitMessage`.
#[must_use]
pub fn run_message_loop() -> i32 {
    let mut msg = MSG::default();
    loop {
        // SAFETY: `msg` is a valid out-pointer; a null HWND reads all
        // messages for this thread.
        let ret = unsafe { GetMessageW(&raw mut msg, None, 0, 0) };
        if ret.0 <= 0 {
            break;
        }
        // SAFETY: `msg` was just filled by GetMessageW.
        unsafe {
            let _ = TranslateMessage(&raw const msg);
            DispatchMessageW(&raw const msg);
        }
    }
    i32::try_from(msg.wParam.0).unwrap_or(1)
}

struct WindowState {
    wake_message: u32,
    handler: Handler,
}

impl WindowState {
    fn dispatch(&self, event: WinEvent) {
        // A re-entrant message (sent while the handler runs) is dropped
        // rather than aliasing the handler.
        if let Ok(mut handler) = self.handler.try_borrow_mut() {
            handler(event);
        }
    }
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_NCCREATE {
        // SAFETY: for WM_NCCREATE, lparam points to the CREATESTRUCTW whose
        // lpCreateParams is the `WindowState` pointer from `create`.
        unsafe {
            let cs = &*(lparam.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
        }
    }
    // SAFETY: GWLP_USERDATA is either 0 or the pointer stored above.
    let state = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const WindowState;

    if !state.is_null() {
        // SAFETY: the pointer stays valid until WM_NCDESTROY frees it below,
        // and this window is only used on its creating thread.
        let state_ref = unsafe { &*state };
        match msg {
            m if m == state_ref.wake_message => {
                state_ref.dispatch(WinEvent::WakeRequested);
                return LRESULT(0);
            }
            WM_DESTROY => {
                // SAFETY: no preconditions.
                unsafe { PostQuitMessage(0) };
                return LRESULT(0);
            }
            WM_NCDESTROY => {
                // SAFETY: last message for this window; we clear the slot
                // and reclaim the Box created in `create`, exactly once.
                unsafe {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    drop(Box::from_raw(state.cast_mut()));
                }
            }
            _ => {}
        }
    }
    // SAFETY: forwarding unmodified arguments to the default procedure.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}
