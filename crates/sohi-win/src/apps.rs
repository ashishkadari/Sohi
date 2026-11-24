//! Finding installed apps: the shell's apps folder (which also has the
//! Store apps) and the `App Paths` registry keys.

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize, IBindCtx,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_ENUMERATE_SUB_KEYS, RegCloseKey,
    RegEnumKeyExW, RegOpenKeyExW,
};
use windows::Win32::UI::Shell::{
    BHID_EnumItems, IEnumShellItems, IShellItem, SHCreateItemFromParsingName, SIGDN,
    SIGDN_NORMALDISPLAY, SIGDN_PARENTRELATIVEPARSING,
};
use windows::core::{PWSTR, w};

use crate::WinError;

/// Runs `work` with COM initialized on this thread. Every COM object must
/// be dropped inside `work`.
pub(crate) fn with_com<T>(work: impl FnOnce() -> Result<T, WinError>) -> Result<T, WinError> {
    // SAFETY: balanced below. A thread already in another apartment
    // refuses; then nothing must be balanced.
    let entered = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
    let result = work();
    if entered {
        // SAFETY: balances CoInitializeEx; every COM object was dropped
        // in `work`.
        unsafe { CoUninitialize() };
    }
    result
}

/// Everything in the shell's apps folder: the name shown in the Start
/// menu, and what to open (`shell:AppsFolder\…`).
///
/// # Errors
/// Returns a [`WinError`] if the folder cannot be read.
pub fn apps_folder() -> Result<Vec<(String, String)>, WinError> {
    with_com(list_apps_folder)
}

fn list_apps_folder() -> Result<Vec<(String, String)>, WinError> {
    // SAFETY: COM is initialized; a constant name and no bind context.
    let folder: IShellItem =
        unsafe { SHCreateItemFromParsingName(w!("shell:AppsFolder"), None::<&IBindCtx>) }
            .map_err(|e| WinError::api("SHCreateItemFromParsingName", &e))?;
    // SAFETY: a live item, a valid handler id and no bind context.
    let items: IEnumShellItems =
        unsafe { folder.BindToHandler(None::<&IBindCtx>, &BHID_EnumItems) }
            .map_err(|e| WinError::api("IShellItem::BindToHandler", &e))?;
    let mut apps = Vec::new();
    loop {
        let mut next = [None];
        // SAFETY: a live enumerator and a one-element out-array.
        unsafe { items.Next(&mut next, None) }
            .map_err(|e| WinError::api("IEnumShellItems::Next", &e))?;
        let [Some(item)] = next else {
            return Ok(apps);
        };
        let name = display_name(&item, SIGDN_NORMALDISPLAY);
        let id = display_name(&item, SIGDN_PARENTRELATIVEPARSING);
        if let (Some(name), Some(id)) = (name, id) {
            apps.push((name, format!("shell:AppsFolder\\{id}")));
        }
    }
}

fn display_name(item: &IShellItem, kind: SIGDN) -> Option<String> {
    // SAFETY: a live item; the returned string is freed below.
    let name: PWSTR = unsafe { item.GetDisplayName(kind) }.ok()?;
    // SAFETY: a NUL-terminated string owned by us.
    let text = unsafe { name.to_string() }.ok();
    // SAFETY: allocated by GetDisplayName with the COM allocator.
    unsafe { CoTaskMemFree(Some(name.0.cast_const().cast())) };
    text
}

/// The programs registered under `App Paths`, e.g. `chrome.exe`.
#[must_use]
pub fn app_paths() -> Vec<String> {
    let mut names = sub_keys(HKEY_CURRENT_USER);
    names.extend(sub_keys(HKEY_LOCAL_MACHINE));
    names
}

fn sub_keys(root: HKEY) -> Vec<String> {
    let mut key = HKEY::default();
    let path = w!("Software\\Microsoft\\Windows\\CurrentVersion\\App Paths");
    // SAFETY: a predefined root, a constant path and a valid out-pointer.
    let opened = unsafe { RegOpenKeyExW(root, path, None, KEY_ENUMERATE_SUB_KEYS, &raw mut key) };
    if opened != ERROR_SUCCESS {
        return Vec::new();
    }
    let mut names = Vec::new();
    for index in 0.. {
        // Key names have at most 255 characters.
        let mut buffer = [0u16; 256];
        let mut len = 256u32;
        let name = PWSTR(buffer.as_mut_ptr());
        // SAFETY: an open key; `name` points at `len` writable units.
        let found =
            unsafe { RegEnumKeyExW(key, index, Some(name), &raw mut len, None, None, None, None) };
        if found != ERROR_SUCCESS {
            break;
        }
        if let Some(name) = buffer.get(..len as usize) {
            names.push(String::from_utf16_lossy(name));
        }
    }
    // SAFETY: opened above and not used after this.
    let _ = unsafe { RegCloseKey(key) };
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_apps_folder_lists_what_to_open() {
        let apps = apps_folder().unwrap();
        assert_ne!(apps.len(), 0);
        for (name, target) in &apps {
            assert_ne!(name, "");
            assert!(target.starts_with("shell:AppsFolder\\"), "{target}");
        }
    }

    #[test]
    fn app_paths_are_programs() {
        let has = |name: &str| app_paths().iter().any(|n| n.eq_ignore_ascii_case(name));
        assert!(has("msedge.exe") || has("iexplore.exe"));
    }
}
