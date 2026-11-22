//! A text prompt for command-line actions such as `--set-key`.
//!
//! Sohi is a GUI-subsystem program, so it has no console of its own. If
//! standard input is a pipe or a file (a script ran
//! `"<key>" | sohi.exe --set-key mistral`), the prompt reads from it.
//! Otherwise it opens a console window of its own. Attaching to the
//! parent's console would race the parent shell for keystrokes, because a
//! shell does not wait for GUI programs.

use std::io::{BufRead, Write};

use sohi_core::secret::Secret;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{FILE_TYPE_DISK, FILE_TYPE_PIPE, GetFileType};
use windows::Win32::System::Console::{
    AllocConsole, CONSOLE_MODE, ENABLE_ECHO_INPUT, FreeConsole, GetConsoleMode, GetStdHandle,
    ReadConsoleW, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetConsoleMode, WriteConsoleW,
};

use crate::WinError;

/// Where the prompt reads and writes.
enum Io {
    /// Redirected standard input; messages go to standard error.
    Redirected,
    /// A console window this process opened (freed on drop).
    Console { input: HANDLE, output: HANDLE },
}

/// A prompt for one command-line action.
pub struct Prompt {
    io: Io,
}

impl Prompt {
    /// Uses redirected standard input if there is one, or opens a console
    /// window.
    ///
    /// # Errors
    /// Returns [`WinError`] if no console can be opened.
    pub fn open() -> Result<Self, WinError> {
        if stdin_is_redirected() {
            return Ok(Self { io: Io::Redirected });
        }
        // SAFETY: no preconditions; fails if this process has a console.
        unsafe { AllocConsole() }.map_err(|e| WinError::api("AllocConsole", &e))?;
        // SAFETY: AllocConsole just set the standard handles.
        let handles = unsafe {
            (
                GetStdHandle(STD_INPUT_HANDLE),
                GetStdHandle(STD_OUTPUT_HANDLE),
            )
        };
        match handles {
            (Ok(input), Ok(output)) => Ok(Self {
                io: Io::Console { input, output },
            }),
            (Err(e), _) | (_, Err(e)) => {
                // SAFETY: we own the console we just allocated.
                let _ = unsafe { FreeConsole() };
                Err(WinError::api("GetStdHandle", &e))
            }
        }
    }

    /// Shows `text` as is (add `\n` for a line break).
    pub fn say(&self, text: &str) {
        match self.io {
            Io::Redirected => {
                let mut err = std::io::stderr();
                let _ = err.write_all(text.as_bytes());
                let _ = err.flush();
            }
            Io::Console { output, .. } => {
                let units: Vec<u16> = text.replace('\n', "\r\n").encode_utf16().collect();
                // SAFETY: `output` is this process's console output handle.
                let _ = unsafe { WriteConsoleW(output, &units, None, None) };
            }
        }
    }

    /// Reads one line without echoing it. Returns `None` for an empty line.
    ///
    /// # Errors
    /// Returns [`WinError`] if reading fails.
    pub fn read_secret(&self) -> Result<Option<Secret>, WinError> {
        match self.io {
            Io::Redirected => {
                let mut line = String::new();
                let read = std::io::stdin().lock().read_line(&mut line);
                let secret = Secret::from_input(&line);
                wipe(line);
                read.map_err(|_| WinError::invalid("ReadFile", "standard input"))?;
                Ok(secret)
            }
            Io::Console { input, .. } => {
                let line = read_console_line(input, false)?;
                self.say("\n"); // with echo off, Enter is not shown either
                let secret = Secret::from_input(&line);
                wipe(line);
                Ok(secret)
            }
        }
    }

    /// In a console window, waits for Enter so the user can read the last
    /// message before the window closes.
    pub fn pause(&self) {
        if let Io::Console { input, .. } = self.io {
            self.say("Press Enter to close this window.");
            if let Ok(line) = read_console_line(input, true) {
                wipe(line);
            }
        }
    }
}

impl Drop for Prompt {
    fn drop(&mut self) {
        if let Io::Console { .. } = self.io {
            // SAFETY: we own the console allocated in `open`.
            let _ = unsafe { FreeConsole() };
        }
    }
}

fn stdin_is_redirected() -> bool {
    // SAFETY: no preconditions.
    let Ok(stdin) = (unsafe { GetStdHandle(STD_INPUT_HANDLE) }) else {
        return false;
    };
    if stdin.is_invalid() {
        return false;
    }
    // SAFETY: `stdin` is a handle the process inherited; GetFileType only
    // queries it.
    let kind = unsafe { GetFileType(stdin) };
    kind == FILE_TYPE_PIPE || kind == FILE_TYPE_DISK
}

/// Reads up to Enter from the console, with echo on or off. The console
/// mode is restored afterwards.
fn read_console_line(input: HANDLE, echo: bool) -> Result<String, WinError> {
    let mut mode = CONSOLE_MODE::default();
    // SAFETY: `input` is this process's console input handle.
    unsafe { GetConsoleMode(input, &raw mut mode) }
        .map_err(|e| WinError::api("GetConsoleMode", &e))?;
    let wanted = if echo {
        mode | ENABLE_ECHO_INPUT
    } else {
        mode & !ENABLE_ECHO_INPUT
    };
    // SAFETY: as above.
    unsafe { SetConsoleMode(input, wanted) }.map_err(|e| WinError::api("SetConsoleMode", &e))?;
    let line = read_units(input);
    // SAFETY: as above; restores the mode read earlier.
    let _ = unsafe { SetConsoleMode(input, mode) };
    line
}

fn read_units(input: HANDLE) -> Result<String, WinError> {
    let mut units: Vec<u16> = Vec::new();
    let mut buffer = [0u16; 256];
    loop {
        let mut read = 0u32;
        // SAFETY: `buffer` has room for the 256 units requested and `read`
        // is a valid out-pointer.
        unsafe { ReadConsoleW(input, buffer.as_mut_ptr().cast(), 256, &raw mut read, None) }
            .map_err(|e| WinError::api("ReadConsoleW", &e))?;
        let got = &buffer[..(read as usize).min(buffer.len())];
        units.extend_from_slice(got);
        if read == 0 || got.contains(&u16::from(b'\n')) {
            break;
        }
    }
    let line = String::from_utf16_lossy(&units);
    buffer.fill(0);
    units.fill(0);
    std::hint::black_box((&buffer, &units));
    Ok(line)
}

/// Zeroes a string that held a secret before freeing it.
fn wipe(text: String) {
    let mut bytes = text.into_bytes();
    bytes.fill(0);
    std::hint::black_box(&bytes);
}
