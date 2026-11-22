//! RAII wrapper for kernel handles.

use windows::Win32::Foundation::{CloseHandle, HANDLE};

/// A kernel handle that is closed on drop.
#[derive(Debug)]
pub struct OwnedHandle(HANDLE);

impl OwnedHandle {
    /// Takes ownership of `handle`.
    ///
    /// # Safety
    /// `handle` must be a valid, open kernel handle that nothing else closes.
    pub(crate) unsafe fn from_raw(handle: HANDLE) -> Self {
        Self(handle)
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: `from_raw` requires a valid handle that only we close, and
        // drop runs exactly once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

// SAFETY: kernel handles are process-wide values, not tied to a thread.
unsafe impl Send for OwnedHandle {}
// SAFETY: `OwnedHandle` exposes no interior mutability; the kernel
// synchronizes operations on the handle itself.
unsafe impl Sync for OwnedHandle {}
