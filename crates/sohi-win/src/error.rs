//! Error type for the FFI layer.

use windows::Win32::Foundation::GetLastError;

/// A failed Win32 or COM call.
#[derive(Debug, thiserror::Error)]
#[error("{api} failed: {message} (0x{code:08X})")]
pub struct WinError {
    api: &'static str,
    code: u32,
    message: String,
}

impl WinError {
    /// Wraps an error returned by the `windows` crate.
    pub(crate) fn api(api: &'static str, err: &windows::core::Error) -> Self {
        Self {
            api,
            code: err.code().0.cast_unsigned(),
            message: err.message(),
        }
    }

    /// Builds an error from the calling thread's last-error value.
    pub(crate) fn last(api: &'static str) -> Self {
        // SAFETY: GetLastError has no preconditions.
        let err = windows::core::Error::from_hresult(unsafe { GetLastError() }.to_hresult());
        Self::api(api, &err)
    }

    /// Name of the failing API.
    #[must_use]
    pub fn api_name(&self) -> &'static str {
        self.api
    }

    /// Short text for the UI; full detail goes to the log via `Display`.
    #[must_use]
    pub fn user_message(&self) -> &'static str {
        "A Windows system call failed. See the log for details."
    }
}
