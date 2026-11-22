//! Loading `%APPDATA%\Sohi\config.toml` (spec §5.7).
//!
//! Loading never fails: a missing file is created with commented defaults,
//! and an unreadable or malformed file is logged and replaced by defaults
//! in memory (the user's file is left untouched).

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use sohi_core::Config;
use sohi_core::config::{DEFAULT_CONFIG_TOML, LogLevel};

/// Default config path: `%APPDATA%\Sohi\config.toml`.
pub fn default_path() -> PathBuf {
    std::env::var_os("APPDATA")
        .map_or_else(std::env::temp_dir, PathBuf::from)
        .join("Sohi")
        .join("config.toml")
}

/// Loads the config at `path`, falling back to defaults on any problem.
pub fn load(path: &Path) -> Config {
    match fs::read_to_string(path) {
        Ok(text) => Config::from_toml_str(&text).unwrap_or_else(|err| {
            log::warn!("{}: {err}; using defaults", path.display());
            Config::default()
        }),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            match write_defaults(path) {
                Ok(()) => log::info!("wrote default config to {}", path.display()),
                Err(err) => log::warn!("cannot write {}: {err}", path.display()),
            }
            Config::default()
        }
        Err(err) => {
            log::warn!("cannot read {}: {err}; using defaults", path.display());
            Config::default()
        }
    }
}

fn write_defaults(path: &Path) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    // create_new: never overwrite a file that appeared in the meantime.
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(DEFAULT_CONFIG_TOML.as_bytes())
}

/// Maps the config's log level to the `log` crate's filter.
pub fn level_filter(level: LogLevel) -> log::LevelFilter {
    match level {
        LogLevel::Error => log::LevelFilter::Error,
        LogLevel::Warn => log::LevelFilter::Warn,
        LogLevel::Info => log::LevelFilter::Info,
        LogLevel::Debug => log::LevelFilter::Debug,
        LogLevel::Trace => log::LevelFilter::Trace,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sohi-cfg-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn missing_file_is_created_with_defaults() {
        let dir = temp_dir("missing");
        let path = dir.join("Sohi").join("config.toml");
        assert_eq!(load(&path), Config::default());
        assert_eq!(fs::read_to_string(&path).unwrap(), DEFAULT_CONFIG_TOML);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn malformed_file_falls_back_and_is_kept() {
        let dir = temp_dir("malformed");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        fs::write(&path, "[wake\n").unwrap();
        assert_eq!(load(&path), Config::default());
        assert_eq!(fs::read_to_string(&path).unwrap(), "[wake\n");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn valid_file_is_used() {
        let dir = temp_dir("valid");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        fs::write(&path, "[ui]\nlaunch_peek = false\n").unwrap();
        assert!(!load(&path).ui.launch_peek);
        fs::remove_dir_all(&dir).unwrap();
    }
}
