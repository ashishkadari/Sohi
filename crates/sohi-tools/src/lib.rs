//! Tool registry, JSON schemas, policy engine and local action executors.

#![forbid(unsafe_code)]

pub mod apps;
mod call;
mod desktop;
mod executor;
#[cfg(test)]
mod fake;
mod registry;

pub use call::{Call, CallError, MediaAction, VolumeAction, one_line};
pub use desktop::{Desktop, WinDesktop};
pub use executor::{Executor, Outcome};
pub use registry::Tool;
