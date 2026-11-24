//! Network connectivity hints (spec §5.3): Windows says when internet
//! access may have come or gone, so nothing has to poll.
//!
//! The callback runs on a system thread; it only posts a message to the
//! app window, which becomes
//! [`WinEvent::NetworkHint`](crate::WinEvent::NetworkHint).

use std::ffi::c_void;

use windows::Win32::Foundation::{HANDLE, NO_ERROR};
use windows::Win32::NetworkManagement::IpHelper::{
    CancelMibChangeNotify2, NotifyNetworkConnectivityHintChange,
};
use windows::Win32::Networking::WinSock::{
    NL_NETWORK_CONNECTIVITY_HINT, NetworkConnectivityLevelHintLocalAccess,
    NetworkConnectivityLevelHintNone,
};

use crate::{RemoteWindow, WinError};

/// A subscription to connectivity hints; ended on drop.
pub struct NetworkWatch {
    handle: HANDLE,
    /// What the callback is given: a box, freed only once it can no
    /// longer run.
    window: *mut RemoteWindow,
}

impl NetworkWatch {
    /// Subscribes `window`. The current state is reported right away.
    ///
    /// # Errors
    /// Returns [`WinError`] if Windows refuses.
    pub fn subscribe(window: RemoteWindow) -> Result<Self, WinError> {
        let window = Box::into_raw(Box::new(window));
        let mut handle = HANDLE::default();
        // SAFETY: the callback has the required signature; `window` is a
        // box that outlives the subscription (freed below or in Drop);
        // `handle` is a valid out-pointer.
        let result = unsafe {
            NotifyNetworkConnectivityHintChange(
                Some(on_hint),
                Some(window.cast_const().cast()),
                true,
                &raw mut handle,
            )
        };
        if result != NO_ERROR {
            // SAFETY: from Box::into_raw above; the callback was not
            // registered.
            drop(unsafe { Box::from_raw(window) });
            return Err(WinError::api(
                "NotifyNetworkConnectivityHintChange",
                &windows::core::Error::from(result.to_hresult()),
            ));
        }
        Ok(Self { handle, window })
    }
}

impl Drop for NetworkWatch {
    fn drop(&mut self) {
        // SAFETY: the handle came from NotifyNetworkConnectivityHintChange.
        // This waits for a callback in progress, so the box outlives it. The
        // callback never waits for this thread (it only posts).
        let _ = unsafe { CancelMibChangeNotify2(self.handle) };
        // SAFETY: from Box::into_raw in `subscribe`; no callback runs any
        // more.
        drop(unsafe { Box::from_raw(self.window) });
    }
}

/// Runs on a system thread.
unsafe extern "system" fn on_hint(context: *const c_void, hint: NL_NETWORK_CONNECTIVITY_HINT) {
    // SAFETY: `context` is the window box given in `subscribe`, which
    // outlives the subscription.
    let Some(window) = (unsafe { context.cast::<RemoteWindow>().as_ref() }) else {
        return;
    };
    let level = hint.ConnectivityLevel;
    // Unknown and hidden levels may well be internet access.
    let none = level == NetworkConnectivityLevelHintNone
        || level == NetworkConnectivityLevelHintLocalAccess;
    // The window may be gone during shutdown.
    let _ = window.post_network_hint(!none);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppWindow, EventQueue};

    #[test]
    fn subscribes_and_unsubscribes() {
        let window = AppWindow::create("Sohi.Test.Net", 0xC150, EventQueue::new()).unwrap();
        let watch = NetworkWatch::subscribe(window.remote()).unwrap();
        assert!(!watch.handle.is_invalid());
        drop(watch);
    }
}
