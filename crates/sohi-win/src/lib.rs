//! All Win32/WinRT FFI for Sohi: windows, message loop, audio, tray, hotkeys and credentials.
//!
//! This is the only crate in the workspace allowed to contain `unsafe` code.
//! Every `unsafe` block needs a `// SAFETY:` comment, and the crate exposes
//! safe RAII wrappers to the rest of the workspace.

mod error;
mod event;
mod handle;
mod head;
mod hwnd;
mod instance;
mod monitor;
mod msgloop;
pub mod perf;
pub mod shell;
mod surface;
mod theme;
mod timer;
mod tray;
mod window;

pub use error::WinError;
pub use event::{EventQueue, WinEvent};
pub use handle::OwnedHandle;
pub use head::{Frame, HeadWindow};
pub use instance::{SingleInstance, forward_to_running_instance, register_window_message};
pub use monitor::{MonitorInfo, primary_monitor};
pub use msgloop::{post_quit, run_message_loop};
pub use theme::{animations_enabled, force_dark_menus};
pub use timer::{FRAME_30_FPS, FRAME_60_FPS, FrameTimer};
pub use tray::{MenuItem, TrayIcon};
pub use window::{AppWindow, RemoteWindow};
