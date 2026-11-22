//! Pure core logic: state machine, events and effects, config schema, intent parser, sentence chunker.

#![forbid(unsafe_code)]

pub mod config;

pub use config::{Config, ConfigError};
