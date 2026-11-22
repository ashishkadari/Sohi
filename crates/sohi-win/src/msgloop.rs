//! The UI-thread message loop (spec §3.2, §4.5 frame pacing).
//!
//! The loop blocks in `MsgWaitForMultipleObjectsEx` with an infinite
//! timeout, so an idle Sohi costs no CPU. It also waits on the
//! [`FrameTimer`], which only signals while an animation runs. After
//! dispatching window messages it drains the [`EventQueue`] into the app's
//! handler.

use windows::Win32::Foundation::{HANDLE, WAIT_FAILED};
use windows::Win32::System::Threading::INFINITE;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MSG, MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_REMOVE,
    PeekMessageW, PostQuitMessage, QS_ALLINPUT, TranslateMessage, WM_QUIT,
};

use crate::{EventQueue, FrameTimer, WinEvent};

/// Runs until `WM_QUIT`, calling `on_event` for every queued event.
/// Each `timer` tick becomes a [`WinEvent::Frame`].
///
/// Returns the exit code passed to [`post_quit`].
pub fn run_message_loop(
    queue: &EventQueue,
    timer: &FrameTimer,
    mut on_event: impl FnMut(WinEvent),
) -> i32 {
    loop {
        if let Some(code) = pump_messages() {
            return code;
        }
        while let Some(event) = queue.pop() {
            on_event(event);
        }
        if wait_for_input(&[timer.raw()]) == Some(0) && timer.on_signaled() {
            queue.push(WinEvent::Frame);
        }
    }
}

/// Dispatches every pending message. Returns the exit code on `WM_QUIT`.
fn pump_messages() -> Option<i32> {
    let mut msg = MSG::default();
    // SAFETY: `msg` is a valid out-pointer; a null HWND reads all messages
    // for this thread.
    while unsafe { PeekMessageW(&raw mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
        if msg.message == WM_QUIT {
            return Some(i32::try_from(msg.wParam.0).unwrap_or(1));
        }
        // SAFETY: `msg` was just filled by PeekMessageW.
        unsafe {
            let _ = TranslateMessage(&raw const msg);
            DispatchMessageW(&raw const msg);
        }
    }
    None
}

/// Blocks until a message arrives or one of `handles` is signaled.
///
/// Returns the index of the signaled handle, or `None` for input.
fn wait_for_input(handles: &[HANDLE]) -> Option<usize> {
    let handles_arg = (!handles.is_empty()).then_some(handles);
    // SAFETY: every handle in `handles` is a valid waitable handle owned by
    // the caller for the duration of the call.
    let ret = unsafe {
        MsgWaitForMultipleObjectsEx(handles_arg, INFINITE, QS_ALLINPUT, MWMO_INPUTAVAILABLE)
    };
    if ret == WAIT_FAILED {
        return None;
    }
    let index = usize::try_from(ret.0).ok()?;
    (index < handles.len()).then_some(index)
}

/// Asks the message loop on this thread to exit with `code`.
pub fn post_quit(code: i32) {
    // SAFETY: no preconditions.
    unsafe { PostQuitMessage(code) };
}

/// Dispatches pending messages without waiting (tests that show windows).
#[cfg(test)]
pub(crate) fn pump_for_tests() {
    let _ = pump_messages();
}
