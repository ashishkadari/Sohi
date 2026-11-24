//! Reading and replacing the clipboard's text.

use std::time::Duration;

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    SetClipboardData,
};
use windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, HWND_MESSAGE, WINDOW_EX_STYLE, WINDOW_STYLE,
};
use windows::core::w;

use crate::WinError;

const CF_UNICODETEXT: u32 = 13;
/// Another program may hold the clipboard for a moment.
const TRIES: u32 = 10;
const TRY_AGAIN: Duration = Duration::from_millis(15);

/// The open clipboard, with a hidden window as its owner: text can only
/// be put on a clipboard that has one.
struct Clipboard {
    owner: windows::Win32::Foundation::HWND,
}

impl Clipboard {
    fn open() -> Result<Self, WinError> {
        // SAFETY: a system class, a message-only parent; no other handles.
        let owner = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                None,
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )
        }
        .map_err(|e| WinError::api("CreateWindowExW", &e))?;
        let mut tries = 1;
        loop {
            // SAFETY: a live window of this thread.
            match unsafe { OpenClipboard(Some(owner)) } {
                Ok(()) => return Ok(Self { owner }),
                Err(_) if tries < TRIES => std::thread::sleep(TRY_AGAIN),
                Err(e) => {
                    // SAFETY: created above on this thread.
                    let _ = unsafe { DestroyWindow(owner) };
                    return Err(WinError::api("OpenClipboard", &e));
                }
            }
            tries += 1;
        }
    }
}

impl Drop for Clipboard {
    fn drop(&mut self) {
        // SAFETY: opened and created by this thread in `open`.
        let _ = unsafe { CloseClipboard() };
        // SAFETY: as above.
        let _ = unsafe { DestroyWindow(self.owner) };
    }
}

/// The text on the clipboard, at most about `max_bytes` of it, or `None`
/// if there is no text.
///
/// # Errors
/// Returns a [`WinError`] if another program keeps the clipboard open.
pub fn clipboard_text(max_bytes: usize) -> Result<Option<String>, WinError> {
    let _open = Clipboard::open()?;
    // SAFETY: no preconditions.
    if unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT) }.is_err() {
        return Ok(None);
    }
    // SAFETY: the clipboard is open; the handle stays valid until it is
    // closed, after the last use below.
    let data = unsafe { GetClipboardData(CF_UNICODETEXT) }
        .map_err(|e| WinError::api("GetClipboardData", &e))?;
    let memory = HGLOBAL(data.0);
    // SAFETY: a valid global memory handle owned by the clipboard.
    let text = unsafe { GlobalLock(memory) }.cast::<u16>();
    if text.is_null() {
        return Err(WinError::last("GlobalLock"));
    }
    // SAFETY: as above. Never more than this is read, NUL or not.
    let units = unsafe { GlobalSize(memory) } / 2;
    // SAFETY: the block is locked, holds `units` u16s, is aligned for
    // them (global memory is 8-byte aligned), and nothing writes to it
    // while the clipboard is open.
    let all = unsafe { std::slice::from_raw_parts(text, units) };
    let end = all.iter().position(|&u| u == 0).unwrap_or(units);
    let shown = all.get(..end.min(max_bytes)).unwrap_or_default();
    let mut text = String::from_utf16_lossy(shown);
    // SAFETY: locked above; the slice is not used after this.
    let _ = unsafe { GlobalUnlock(memory) };
    truncate(&mut text, max_bytes);
    Ok(Some(text))
}

/// Cuts `text` to `max` bytes at a character boundary.
fn truncate(text: &mut String, max: usize) {
    while text.len() > max {
        let cut = (0..=max).rev().find(|&i| text.is_char_boundary(i));
        text.truncate(cut.unwrap_or(0));
    }
}

/// Puts `text` on the clipboard, replacing what is there.
///
/// # Errors
/// Returns a [`WinError`] if the clipboard cannot be written.
pub fn set_clipboard_text(text: &str) -> Result<(), WinError> {
    let units: Vec<u16> = text.encode_utf16().chain([0]).collect();
    let _open = Clipboard::open()?;
    // SAFETY: the clipboard is open and has an owner.
    unsafe { EmptyClipboard() }.map_err(|e| WinError::api("EmptyClipboard", &e))?;
    // SAFETY: plain allocation; freed below unless the clipboard takes it.
    let memory = unsafe { GlobalAlloc(GMEM_MOVEABLE, units.len() * 2) }
        .map_err(|e| WinError::api("GlobalAlloc", &e))?;
    let given = fill(memory, &units).and_then(|()| {
        // SAFETY: the clipboard is open; it owns the memory from here on.
        unsafe { SetClipboardData(CF_UNICODETEXT, Some(HANDLE(memory.0))) }
            .map_err(|e| WinError::api("SetClipboardData", &e))
    });
    match given {
        Ok(_) => Ok(()),
        Err(err) => {
            // SAFETY: the clipboard did not take it, so it is still ours.
            let _ = unsafe { GlobalFree(Some(memory)) };
            Err(err)
        }
    }
}

/// Copies `units` into `memory`, which is that large.
fn fill(memory: HGLOBAL, units: &[u16]) -> Result<(), WinError> {
    // SAFETY: a live movable block, unlocked below.
    let to = unsafe { GlobalLock(memory) }.cast::<u16>();
    if to.is_null() {
        return Err(WinError::last("GlobalLock"));
    }
    // SAFETY: the block holds `units.len()` u16s, aligned, and is not
    // shared with anyone yet.
    unsafe { std::ptr::copy_nonoverlapping(units.as_ptr(), to, units.len()) };
    // SAFETY: locked above. (It reports "not locked any more" as an error.)
    let _ = unsafe { GlobalUnlock(memory) };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_between_characters() {
        let mut text = "aé€".to_owned();
        truncate(&mut text, 5);
        assert_eq!(text, "aé");
        truncate(&mut text, 2);
        assert_eq!(text, "a");
        truncate(&mut text, 0);
        assert_eq!(text, "");
    }

    #[test]
    #[ignore = "replaces what is on the clipboard"]
    fn round_trip() {
        let before = clipboard_text(1 << 20).unwrap();
        set_clipboard_text("héllo\r\n€").unwrap();
        assert_eq!(clipboard_text(100).unwrap().as_deref(), Some("héllo\r\n€"));
        assert_eq!(clipboard_text(3).unwrap().as_deref(), Some("hé"));
        if let Some(text) = before {
            set_clipboard_text(&text).unwrap();
        }
    }
}
