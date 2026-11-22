//! All Win32/WinRT FFI for Sohi: windows, message loop, audio, tray, hotkeys and credentials.
//!
//! This is the only crate in the workspace allowed to contain `unsafe` code.
//! Every `unsafe` block needs a `// SAFETY:` comment, and the crate exposes
//! safe RAII wrappers to the rest of the workspace.
