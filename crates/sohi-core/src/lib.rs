//! Pure core logic: state machine, events and effects, config schema, intent parser, sentence chunker.

#![forbid(unsafe_code)]

pub mod config;
pub mod geometry;
pub mod motion;
pub mod visibility;
pub mod visual;

pub use config::{Config, ConfigError};
