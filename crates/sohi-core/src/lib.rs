//! Pure core logic: state machine, events and effects, config schema, intent parser, sentence chunker.

#![forbid(unsafe_code)]

pub mod chat;
pub mod config;
pub mod connectivity;
pub mod geometry;
pub mod history;
pub mod hotkey;
pub mod interaction;
pub mod meter;
pub mod motion;
pub mod persona;
pub mod pill;
pub mod secret;
pub mod speech;
pub mod visibility;
pub mod visual;
pub mod wake;

pub use config::{Config, ConfigError};
