//! All Win32/WinRT FFI for Sohi: windows, message loop, audio, tray, hotkeys and credentials.
//!
//! This is the only crate in the workspace allowed to contain `unsafe` code.
//! Every `unsafe` block needs a `// SAFETY:` comment, and the crate exposes
//! safe RAII wrappers to the rest of the workspace.

mod apps;
mod apptimer;
mod audio;
mod capture;
mod clipboard;
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
#[cfg(feature = "shell")]
mod job;
mod monitor;
mod msgloop;
mod network;
pub mod perf;
mod pill;
mod pill_proc;
mod render;
mod session;
pub mod shell;
mod speech;
mod surface;
mod theme;
mod timer;
mod tray;
mod volume;
mod window;

pub use apps::{app_paths, apps_folder};
pub use apptimer::{AppTimer, TimerHost};
pub use capture::Capture;
pub use clipboard::{clipboard_text, set_clipboard_text};
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
#[cfg(feature = "shell")]
pub use job::Job;
pub use monitor::{MonitorInfo, primary_monitor};
pub use msgloop::{post_quit, run_message_loop};
pub use network::NetworkWatch;
pub use pill::PillWindow;
pub use render::Playback;
pub use session::{Presence, PresenceWatch};
pub use speech::synthesize_offline;
pub use theme::{animations_enabled, force_dark_menus};
pub use timer::{FRAME_30_FPS, FRAME_60_FPS, FrameTimer};
pub use tray::{MenuItem, TrayIcon};
pub use volume::{MediaKey, press_media_key, set_volume, volume};
pub use window::{AppWindow, RemoteWindow};
