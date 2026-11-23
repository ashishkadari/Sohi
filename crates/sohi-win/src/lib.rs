//! All Win32/WinRT FFI for Sohi: windows, message loop, audio, tray, hotkeys and credentials.
//!
//! This is the only crate in the workspace allowed to contain `unsafe` code.
//! Every `unsafe` block needs a `// SAFETY:` comment, and the crate exposes
//! safe RAII wrappers to the rest of the workspace.

mod apptimer;
mod capture;
mod clock;
mod console;
mod credential;
mod cursor;
mod error;
mod event;
mod gdi;
mod handle;
mod head;
mod hotkey;
mod hwnd;
mod instance;
mod monitor;
mod msgloop;
pub mod perf;
mod pill;
mod pill_proc;
mod session;
pub mod shell;
mod surface;
mod theme;
mod timer;
mod tray;
mod window;

pub use apptimer::{AppTimer, TimerHost};
pub use capture::Capture;
pub use clock::local_time;
pub use console::Prompt;
pub use credential::{delete_key, read_key, write_key};
pub use cursor::{CursorPoller, cursor_pos, mouse_button_down};
pub use error::WinError;
pub use event::{EventQueue, WinEvent};
pub use handle::OwnedHandle;
pub use head::{Frame, HeadWindow};
pub use hotkey::{HotkeyHost, HotkeyRegistration};
pub use instance::{SingleInstance, forward_to_running_instance, register_window_message};
pub use monitor::{MonitorInfo, primary_monitor};
pub use msgloop::{post_quit, run_message_loop};
pub use pill::PillWindow;
pub use session::{Presence, PresenceWatch};
pub use theme::{animations_enabled, force_dark_menus};
pub use timer::{FRAME_30_FPS, FRAME_60_FPS, FrameTimer};
pub use tray::{MenuItem, TrayIcon};
pub use window::{AppWindow, RemoteWindow};
