//! Hook execution system for git-mile
//!
//! This crate provides functionality to execute scripts (hooks) before and after
//! git-mile events, similar to Git hooks.

mod config;
mod error;
mod executor;
mod types;

pub use config::HooksConfig;
pub use error::{HookError, Result};
pub use executor::HookExecutor;
pub use types::{HookContext, HookKind, HookResult};
