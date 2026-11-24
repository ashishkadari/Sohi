//! Remembers "Force offline" across restarts: an empty file next to the
//! config that exists while it is on. The config itself is the user's and
//! is never written.

use std::io::ErrorKind;
use std::path::Path;

/// The file's name.
pub const NAME: &str = "force-offline";

/// Creates or removes the flag.
///
/// # Errors
/// Returns the I/O error, if any.
pub fn set(path: &Path, on: bool) -> std::io::Result<()> {
    if on {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        return std::fs::write(path, []);
    }
    match std::fs::remove_file(path) {
        Err(err) if err.kind() != ErrorKind::NotFound => Err(err),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flag_comes_and_goes() {
        let dir = std::env::temp_dir().join(format!("sohi-flag-{}", std::process::id()));
        let path = dir.join(NAME);
        set(&path, false).unwrap();
        set(&path, true).unwrap();
        assert!(path.exists());
        set(&path, true).unwrap();
        set(&path, false).unwrap();
        assert!(!path.exists());
        std::fs::remove_dir(&dir).unwrap();
    }
}
