//! All Win32/WinRT FFI for Sohi: windows, message loop, audio, tray, hotkeys and credentials.
//!
//! This is the only crate in the workspace allowed to contain `unsafe` code.
//! Every `unsafe` block needs a `// SAFETY:` comment, and the crate exposes
//! safe RAII wrappers to the rest of the workspace.

mod error;
mod handle;
mod instance;
mod window;

pub use error::WinError;
pub use handle::OwnedHandle;
pub use instance::{SingleInstance, forward_to_running_instance, register_window_message};
pub use window::{AppWindow, RemoteWindow, WinEvent, run_message_loop};
