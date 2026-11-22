//! Notification-area icon and its popup menu (spec §4.8).
//!
//! The icon reports to the hidden [`AppWindow`] through [`TRAY_CALLBACK`]
//! (version 4 semantics). `TaskbarCreated` (Explorer restarted) is turned
//! into [`WinEvent::TaskbarCreated`] so the app can call [`TrayIcon::readd`].

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::HiDpi::GetSystemMetricsForDpi;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION,
    NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIcon, CreatePopupMenu, DestroyIcon, DestroyMenu, HICON, MENU_ITEM_FLAGS,
    MF_CHECKED, MF_GRAYED, MF_SEPARATOR, MF_STRING, PostMessageW, SM_CXSMICON, SetForegroundWindow,
    TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx, WM_APP, WM_NULL,
};
use windows::core::HSTRING;

use crate::hwnd::module_instance;
use crate::{AppWindow, WinError};

/// Message the shell posts to the app window for icon events.
pub(crate) const TRAY_CALLBACK: u32 = WM_APP + 1;
const TRAY_ID: u32 = 1;
const TOOLTIP: &str = "Sohi";

/// One popup-menu entry.
#[derive(Debug, Clone, Copy)]
pub enum MenuItem<'a> {
    /// A command. `checked: Some(_)` draws a check box.
    Command {
        /// Identifier returned by [`TrayIcon::show_menu`]; must be non-zero.
        id: u16,
        /// Text shown in the menu.
        label: &'a str,
        /// Greyed out when `false`.
        enabled: bool,
        /// Check-box state, if the item is a toggle.
        checked: Option<bool>,
    },
    /// A divider line.
    Separator,
}

/// The tray icon. Removed from the notification area on drop.
pub struct TrayIcon {
    hwnd: HWND,
    icon: HICON,
}

impl TrayIcon {
    /// Adds the icon, drawn from `rgba_for_size(size)` at the small-icon
    /// size for `dpi`.
    ///
    /// # Errors
    /// Returns [`WinError`] if the icon cannot be created or added.
    pub fn add(
        window: &AppWindow,
        dpi: u32,
        rgba_for_size: impl Fn(u32) -> Vec<u8>,
    ) -> Result<Self, WinError> {
        let icon = create_icon(dpi, &rgba_for_size)?;
        let tray = Self {
            hwnd: window.raw(),
            icon,
        };
        tray.notify(NIM_ADD)?;
        let mut data = tray.data();
        data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        // SAFETY: `data` is a fully initialized NOTIFYICONDATAW.
        if !unsafe { Shell_NotifyIconW(NIM_SETVERSION, &raw const data) }.as_bool() {
            return Err(WinError::last("Shell_NotifyIconW(NIM_SETVERSION)"));
        }
        Ok(tray)
    }

    /// Re-adds the icon after Explorer restarts (`TaskbarCreated`).
    ///
    /// # Errors
    /// Returns [`WinError`] if the shell rejects the icon.
    pub fn readd(&self) -> Result<(), WinError> {
        // After a real Explorer restart the icon is gone and NIM_ADD works;
        // if it still exists (spurious broadcast), refresh it instead.
        self.notify(NIM_ADD).or_else(|_| self.notify(NIM_MODIFY))?;
        let mut data = self.data();
        data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        // SAFETY: `data` is a fully initialized NOTIFYICONDATAW.
        let _ = unsafe { Shell_NotifyIconW(NIM_SETVERSION, &raw const data) };
        Ok(())
    }

    /// Redraws the icon for a new DPI.
    ///
    /// # Errors
    /// Returns [`WinError`] if the icon cannot be created or updated.
    pub fn set_dpi(
        &mut self,
        dpi: u32,
        rgba_for_size: impl Fn(u32) -> Vec<u8>,
    ) -> Result<(), WinError> {
        let icon = create_icon(dpi, &rgba_for_size)?;
        let old = std::mem::replace(&mut self.icon, icon);
        let result = self.notify(NIM_MODIFY);
        // SAFETY: `old` was created by `create_icon` and is no longer used.
        let _ = unsafe { DestroyIcon(old) };
        result
    }

    /// Shows `items` as a popup menu at screen point (`x`, `y`) and returns
    /// the chosen item's id, or `None` if the menu was dismissed.
    #[must_use]
    pub fn show_menu(&self, items: &[MenuItem<'_>], x: i32, y: i32) -> Option<u16> {
        // SAFETY: creating an empty popup menu has no preconditions.
        let menu = unsafe { CreatePopupMenu() }.ok()?;
        for item in items {
            append_item(menu, item);
        }
        // SAFETY: valid menu and window. The window must be foreground for
        // the menu to close when the user clicks elsewhere; the WM_NULL
        // post afterwards is the documented companion to that.
        let chosen = unsafe {
            let _ = SetForegroundWindow(self.hwnd);
            let cmd = TrackPopupMenuEx(
                menu,
                (TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON).0,
                x,
                y,
                self.hwnd,
                None,
            );
            let _ = PostMessageW(Some(self.hwnd), WM_NULL, WPARAM(0), LPARAM(0));
            let _ = DestroyMenu(menu);
            cmd.0
        };
        u16::try_from(chosen).ok().filter(|&id| id != 0)
    }

    fn data(&self) -> NOTIFYICONDATAW {
        let mut data = NOTIFYICONDATAW {
            cbSize: u32::try_from(size_of::<NOTIFYICONDATAW>()).unwrap_or(0),
            hWnd: self.hwnd,
            uID: TRAY_ID,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP,
            uCallbackMessage: TRAY_CALLBACK,
            hIcon: self.icon,
            ..Default::default()
        };
        for (dst, src) in data.szTip.iter_mut().zip(TOOLTIP.encode_utf16()) {
            *dst = src;
        }
        data
    }

    fn notify(
        &self,
        action: windows::Win32::UI::Shell::NOTIFY_ICON_MESSAGE,
    ) -> Result<(), WinError> {
        let data = self.data();
        // SAFETY: `data` is a fully initialized NOTIFYICONDATAW for our window.
        if unsafe { Shell_NotifyIconW(action, &raw const data) }.as_bool() {
            Ok(())
        } else {
            Err(WinError::last("Shell_NotifyIconW"))
        }
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        let _ = self.notify(NIM_DELETE);
        // SAFETY: the icon was created by `create_icon` and the shell no
        // longer references it after NIM_DELETE.
        let _ = unsafe { DestroyIcon(self.icon) };
    }
}

fn append_item(menu: windows::Win32::UI::WindowsAndMessaging::HMENU, item: &MenuItem<'_>) {
    match *item {
        MenuItem::Separator => {
            // SAFETY: valid menu; separators take no text.
            let _ = unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, None) };
        }
        MenuItem::Command {
            id,
            label,
            enabled,
            checked,
        } => {
            let mut flags: MENU_ITEM_FLAGS = MF_STRING;
            if !enabled {
                flags |= MF_GRAYED;
            }
            if checked == Some(true) {
                flags |= MF_CHECKED;
            }
            let text = HSTRING::from(label);
            // SAFETY: valid menu; `text` outlives the call (the menu copies it).
            let _ = unsafe { AppendMenuW(menu, flags, usize::from(id), &text) };
        }
    }
}

/// Builds an HICON from square RGBA (straight or premultiplied alpha both
/// work for opaque-or-clear pixels) at the small-icon size for `dpi`.
fn create_icon(dpi: u32, rgba_for_size: &dyn Fn(u32) -> Vec<u8>) -> Result<HICON, WinError> {
    // SAFETY: no preconditions.
    let size = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, dpi) };
    let size = u32::try_from(size).ok().filter(|&s| s > 0).unwrap_or(16);
    let rgba = rgba_for_size(size);
    if rgba.len() != (size * size * 4) as usize {
        return Err(WinError::invalid("CreateIcon", "icon pixels"));
    }
    let (xor, and) = icon_bits(&rgba, size);
    let edge_px = i32::try_from(size).unwrap_or(16);
    // SAFETY: `and` is a 1-bpp mask with WORD-aligned rows and `xor` holds
    // 32-bpp BGRA rows, both exactly sized for `edge_px` x `edge_px`.
    unsafe {
        CreateIcon(
            Some(module_instance()?),
            edge_px,
            edge_px,
            1,
            32,
            and.as_ptr(),
            xor.as_ptr(),
        )
    }
    .map_err(|e| WinError::api("CreateIcon", &e))
}

/// Converts RGBA into the BGRA colour plane and the 1-bpp AND mask
/// (bit set = transparent) that `CreateIcon` expects.
fn icon_bits(rgba: &[u8], size: u32) -> (Vec<u8>, Vec<u8>) {
    let size = size as usize;
    let mask_stride = size.div_ceil(16) * 2; // rows padded to 16 bits
    let mut xor = vec![0u8; size * size * 4];
    let mut and = vec![0u8; mask_stride * size];
    for (i, (&[r, g, b, a], dst)) in rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(xor.as_chunks_mut::<4>().0)
        .enumerate()
    {
        *dst = [b, g, r, a];
        if a == 0 {
            let (x, y) = (i % size, i / size);
            and[y * mask_stride + x / 8] |= 0x80 >> (x % 8);
        }
    }
    (xor, and)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_bits_swizzle_and_mask_transparent_pixels() {
        // 2x2 at a padded 16-px stride: opaque red, clear, clear, opaque blue.
        let rgba = [255, 0, 0, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 255];
        let (xor, and) = icon_bits(&rgba, 2);
        assert_eq!(&xor[0..4], &[0, 0, 255, 255]);
        assert_eq!(&xor[12..16], &[255, 0, 0, 255]);
        assert_eq!(and.len(), 2 * 2);
        assert_eq!(and[0], 0b0100_0000); // row 0: x=1 clear
        assert_eq!(and[2], 0b1000_0000); // row 1: x=0 clear
    }

    #[test]
    fn mask_rows_are_word_aligned() {
        let rgba = vec![0u8; 20 * 20 * 4];
        let (_, and) = icon_bits(&rgba, 20);
        assert_eq!(and.len(), 4 * 20);
    }
}
