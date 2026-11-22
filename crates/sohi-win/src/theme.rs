//! Dark popup menus (spec §4.8).
//!
//! Win32 popup menus follow the per-app "preferred app mode" kept by
//! uxtheme.dll. The setter is exported only by ordinal and is undocumented,
//! but it has been stable since Windows 10 1903 and is what Windows Terminal
//! and Notepad++ use. If the ordinals can't be resolved, menus keep the
//! system theme.

use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows::core::{PCSTR, w};

/// `SetPreferredAppMode` (uxtheme ordinal 135).
const ORD_SET_PREFERRED_APP_MODE: usize = 135;
/// `FlushMenuThemes` (uxtheme ordinal 136).
const ORD_FLUSH_MENU_THEMES: usize = 136;
/// `PreferredAppMode::ForceDark`.
const APP_MODE_FORCE_DARK: i32 = 2;

type SetPreferredAppMode = unsafe extern "system" fn(i32) -> i32;
type FlushMenuThemes = unsafe extern "system" fn();

/// Makes this process's popup menus dark regardless of the Windows app
/// theme. Returns `false` (and changes nothing) if uxtheme lacks the
/// entry points.
#[must_use = "false means menus keep the system theme"]
pub fn force_dark_menus() -> bool {
    // SAFETY: loads a system DLL from System32 only; the module stays loaded
    // for the life of the process (uxtheme is already mapped by comctl32).
    let Ok(uxtheme) =
        (unsafe { LoadLibraryExW(w!("uxtheme.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32) })
    else {
        return false;
    };
    // SAFETY: lookup by ordinal; a PCSTR whose value is below 0x10000 is
    // interpreted as an ordinal, never dereferenced.
    let (set, flush) = unsafe {
        (
            GetProcAddress(
                uxtheme,
                PCSTR(std::ptr::without_provenance(ORD_SET_PREFERRED_APP_MODE)),
            ),
            GetProcAddress(
                uxtheme,
                PCSTR(std::ptr::without_provenance(ORD_FLUSH_MENU_THEMES)),
            ),
        )
    };
    let (Some(set), Some(flush)) = (set, flush) else {
        return false;
    };
    // SAFETY: on Windows 10 1903 and later (Sohi targets Windows 11) these
    // ordinals have exactly these signatures.
    unsafe {
        let set: SetPreferredAppMode = std::mem::transmute(set);
        let flush: FlushMenuThemes = std::mem::transmute(flush);
        set(APP_MODE_FORCE_DARK);
        flush();
    }
    true
}
