//! Tool registry, JSON schemas, policy engine and local action executors.

#![forbid(unsafe_code)]

pub mod apps;
mod call;
mod desktop;
mod executor;
#[cfg(test)]
mod fake;
mod policy;
mod registry;
#[cfg(feature = "shell")]
pub mod shell;
pub mod url;

pub use call::{Call, CallError, MediaAction, VolumeAction, one_line};
pub use desktop::{Desktop, WinDesktop};
pub use executor::{Executor, Outcome};
pub use policy::{Gate, Permit, Policy, Question, Verdict};
pub use registry::Tool;
