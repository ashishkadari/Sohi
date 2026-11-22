//! Pure core logic: state machine, events and effects, config schema, intent parser, sentence chunker.

#![forbid(unsafe_code)]

pub mod chat;
pub mod config;
pub mod geometry;
pub mod history;
pub mod hotkey;
pub mod interaction;
pub mod motion;
pub mod persona;
pub mod pill;
pub mod secret;
pub mod visibility;
pub mod visual;
pub mod wake;

pub use config::{Config, ConfigError};
