//! API keys in Windows Credential Manager (spec §5.7, §9.5).
//!
//! Each key is a generic credential with the key's UTF-8 bytes as its
//! blob, persisted for this user on this machine. It shows up in Control
//! Panel → Credential Manager → Windows Credentials, where the user can
//! also remove it.

use sohi_core::secret::Secret;
use windows::Win32::Foundation::ERROR_NOT_FOUND;
use windows::Win32::Security::Credentials::{
    CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree, CredReadW,
    CredWriteW,
};
use windows::core::{HSTRING, PWSTR};

use crate::WinError;

/// Largest blob Credential Manager stores (`CRED_MAX_CREDENTIAL_BLOB_SIZE`).
const MAX_BLOB: usize = 5 * 512;

/// Stores `secret` under `target`, replacing any earlier value.
///
/// # Errors
/// Returns [`WinError`] if the key is too long or Credential Manager
/// refuses it.
pub fn write_key(target: &str, secret: &Secret) -> Result<(), WinError> {
    let blob = secret.expose().as_bytes();
    if blob.len() > MAX_BLOB {
        return Err(WinError::invalid("CredWriteW", "key length"));
    }
    let mut target = wide(target);
    let mut user = wide("Sohi");
    let credential = CREDENTIALW {
        Type: CRED_TYPE_GENERIC,
        TargetName: PWSTR(target.as_mut_ptr()),
        UserName: PWSTR(user.as_mut_ptr()),
        CredentialBlobSize: u32::try_from(blob.len()).unwrap_or(u32::MAX),
        // CredWriteW only reads the blob.
        CredentialBlob: blob.as_ptr().cast_mut(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        ..CREDENTIALW::default()
    };
    // SAFETY: every pointer in `credential` points into `target`, `user`
    // or `blob`, which outlive the call; the sizes match the buffers.
    unsafe { CredWriteW(&raw const credential, 0) }.map_err(|e| WinError::api("CredWriteW", &e))
}

/// Reads the key stored under `target`, or `None` if there is none.
///
/// # Errors
/// Returns [`WinError`] if Credential Manager fails, or if the stored
/// value is not UTF-8 (someone else wrote it).
pub fn read_key(target: &str) -> Result<Option<Secret>, WinError> {
    let mut credential: *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: `credential` is a valid out-pointer; on success it receives
    // a buffer that `Freed` releases with CredFree.
    let read = unsafe {
        CredReadW(
            &HSTRING::from(target),
            CRED_TYPE_GENERIC,
            None,
            &raw mut credential,
        )
    };
    match read {
        Err(e) if e.code() == ERROR_NOT_FOUND.to_hresult() => return Ok(None),
        Err(e) => return Err(WinError::api("CredReadW", &e)),
        Ok(()) => {}
    }
    let freed = Freed(credential);
    // SAFETY: CredReadW succeeded, so `credential` points to a valid
    // CREDENTIALW whose blob holds `CredentialBlobSize` bytes; both stay
    // valid until `freed` drops.
    let blob = unsafe {
        let c = &*freed.0;
        if c.CredentialBlob.is_null() {
            &[][..]
        } else {
            std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize)
        }
    };
    let text = std::str::from_utf8(blob).map_err(|_| WinError::invalid("CredReadW", "key"))?;
    Ok(Secret::from_input(text))
}

/// Removes the key stored under `target`. Removing a missing key is not
/// an error.
///
/// # Errors
/// Returns [`WinError`] if Credential Manager fails.
pub fn delete_key(target: &str) -> Result<(), WinError> {
    // SAFETY: the target string outlives the call.
    match unsafe { CredDeleteW(&HSTRING::from(target), CRED_TYPE_GENERIC, None) } {
        Err(e) if e.code() == ERROR_NOT_FOUND.to_hresult() => Ok(()),
        other => other.map_err(|e| WinError::api("CredDeleteW", &e)),
    }
}

/// Frees a buffer returned by `CredReadW`.
struct Freed(*mut CREDENTIALW);

impl Drop for Freed {
    fn drop(&mut self) {
        // SAFETY: the pointer came from a successful CredReadW and is freed
        // exactly once.
        unsafe { CredFree(self.0.cast_const().cast()) };
    }
}

/// A NUL-terminated UTF-16 copy of `s`.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_read_and_delete_round_trip() {
        let target = format!("Sohi.Test/credential-{}", std::process::id());
        assert!(read_key(&target).unwrap().is_none());

        write_key(&target, &Secret::from_input("first-key").unwrap()).unwrap();
        write_key(&target, &Secret::from_input("second-key ünï").unwrap()).unwrap();
        let read = read_key(&target).unwrap().unwrap();
        assert_eq!(read.expose(), "second-key ünï");

        delete_key(&target).unwrap();
        assert!(read_key(&target).unwrap().is_none());
        delete_key(&target).unwrap(); // already gone: still fine
    }

    #[test]
    fn oversized_keys_are_refused() {
        let huge = "k".repeat(MAX_BLOB + 1);
        let err = write_key("Sohi.Test/huge", &Secret::from_input(&huge).unwrap());
        assert!(err.is_err());
    }
}
