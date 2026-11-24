//! Starting with Windows: a value under `HKCU\…\Run`.

use std::path::Path;

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, REG_VALUE_TYPE, RRF_RT_REG_BINARY, RRF_RT_REG_SZ,
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows::core::HSTRING;

use crate::WinError;

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// Where Task Manager and Settings note what the user switched off there.
const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
/// Room for a command line.
const MAX_UNITS: usize = 1024;

/// Whether Windows starts `exe` as `name` when the user signs in.
#[must_use]
pub fn starts_with_windows(name: &str, exe: &Path) -> bool {
    Keys::REAL.is_on(name, exe)
}

/// Has Windows start `exe` as `name` when the user signs in, or not.
///
/// # Errors
/// Returns a [`WinError`] if the registry refuses.
pub fn set_starts_with_windows(name: &str, exe: &Path, on: bool) -> Result<(), WinError> {
    Keys::REAL.set(name, exe, on)
}

/// Keys under `HKCU`; tests have their own.
#[derive(Clone, Copy)]
struct Keys<'a> {
    run: &'a str,
    approved: &'a str,
}

impl Keys<'_> {
    const REAL: Keys<'static> = Keys {
        run: RUN,
        approved: APPROVED,
    };

    fn is_on(self, name: &str, exe: &Path) -> bool {
        // Another copy of Sohi may be registered: that is not this one.
        let registered =
            text(self.run, name).is_some_and(|c| c.eq_ignore_ascii_case(&command(exe)));
        // An odd first byte means the user switched it off in Settings.
        let off = first_byte(self.approved, name).is_some_and(|byte| byte & 1 == 1);
        registered && !off
    }

    fn set(self, name: &str, exe: &Path, on: bool) -> Result<(), WinError> {
        // Either way what was chosen in Settings before is over.
        delete(self.approved, name)?;
        if !on {
            return delete(self.run, name);
        }
        let wide: Vec<u16> = command(exe).encode_utf16().chain([0]).collect();
        let bytes = u32::try_from(wide.len() * 2)
            .map_err(|_| WinError::invalid("RegSetKeyValueW", "path"))?;
        // SAFETY: NUL-terminated names, and `bytes` bytes of NUL-terminated
        // text that outlive the call.
        let result = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                &HSTRING::from(self.run),
                &HSTRING::from(name),
                REG_SZ.0,
                Some(wide.as_ptr().cast()),
                bytes,
            )
        };
        check("RegSetKeyValueW", result)
    }
}

/// The command line that starts `exe`. Quoted, or a path with spaces could
/// start something else.
fn command(exe: &Path) -> String {
    format!("\"{}\"", exe.display())
}

fn check(api: &'static str, result: WIN32_ERROR) -> Result<(), WinError> {
    if result == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(WinError::api(api, &result.to_hresult().into()))
    }
}

fn delete(key: &str, name: &str) -> Result<(), WinError> {
    // SAFETY: NUL-terminated names that outlive the call.
    let result =
        unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(key), &HSTRING::from(name)) };
    if result == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    check("RegDeleteKeyValueW", result)
}

fn text(key: &str, name: &str) -> Option<String> {
    let mut units = [0u16; MAX_UNITS];
    let mut bytes = u32::try_from(size_of_val(&units)).ok()?;
    // SAFETY: NUL-terminated names; `units` has room for `bytes` bytes.
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(key),
            &HSTRING::from(name),
            RRF_RT_REG_SZ,
            None::<*mut REG_VALUE_TYPE>,
            Some(units.as_mut_ptr().cast()),
            Some(&raw mut bytes),
        )
    };
    if result != ERROR_SUCCESS {
        return None;
    }
    let len = units.iter().position(|&u| u == 0)?;
    String::from_utf16(units.get(..len)?).ok()
}

fn first_byte(key: &str, name: &str) -> Option<u8> {
    let mut data = [0u8; 64];
    let mut bytes = u32::try_from(data.len()).ok()?;
    // SAFETY: NUL-terminated names; `data` has room for `bytes` bytes.
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(key),
            &HSTRING::from(name),
            RRF_RT_REG_BINARY,
            None::<*mut REG_VALUE_TYPE>,
            Some(data.as_mut_ptr().cast()),
            Some(&raw mut bytes),
        )
    };
    (result == ERROR_SUCCESS && bytes > 0).then_some(data[0])
}

#[cfg(test)]
mod tests {
    use windows::Win32::System::Registry::{REG_BINARY, RegDeleteTreeW};

    use super::*;

    /// Never the user's real startup list.
    #[test]
    fn it_is_switched_on_and_off() {
        let base = format!(r"Software\Sohi.Test.{}", std::process::id());
        let (run, approved) = (format!(r"{base}\Run"), format!(r"{base}\Approved"));
        let keys = Keys {
            run: &run,
            approved: &approved,
        };
        let exe = Path::new(r"C:\Program Files\Sohi\sohi.exe");
        assert!(!keys.is_on("Sohi", exe));
        keys.set("Sohi", exe, false).unwrap();
        keys.set("Sohi", exe, true).unwrap();
        assert!(keys.is_on("Sohi", Path::new(r"c:\program files\sohi\SOHI.exe")));
        assert!(!keys.is_on("Sohi", Path::new(r"D:\sohi.exe")));
        assert_eq!(
            text(&run, "Sohi").unwrap(),
            "\"C:\\Program Files\\Sohi\\sohi.exe\""
        );

        // Switched off in Settings, then on again here.
        let off = [3u8, 0, 0, 0];
        // SAFETY: NUL-terminated names and 4 readable bytes.
        let set = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                &HSTRING::from(&approved),
                &HSTRING::from("Sohi"),
                REG_BINARY.0,
                Some(off.as_ptr().cast()),
                4,
            )
        };
        assert_eq!(set, ERROR_SUCCESS);
        assert!(!keys.is_on("Sohi", exe));
        keys.set("Sohi", exe, true).unwrap();
        assert!(keys.is_on("Sohi", exe));

        keys.set("Sohi", exe, false).unwrap();
        assert!(!keys.is_on("Sohi", exe));
        // SAFETY: a NUL-terminated name; only this test uses the key.
        let _ = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(&base)) };
    }
}
