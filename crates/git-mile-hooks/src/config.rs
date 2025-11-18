//! Hook configuration

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Configuration for hook execution
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HooksConfig {
    /// Whether hooks are enabled
    pub enabled: bool,

    /// List of disabled hook names
    pub disabled: Vec<String>,

    /// Timeout in seconds for hook execution
    pub timeout: u64,

    /// Whether to run post-hooks asynchronously
    pub async_post_hooks: bool,

    /// Directory containing hook scripts (relative to .git-mile/)
    pub hooks_dir: PathBuf,
}

impl Default for HooksConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            disabled: Vec::new(),
            timeout: 30,
            async_post_hooks: true,
            hooks_dir: PathBuf::from("hooks"),
        }
    }
}

impl HooksConfig {
    /// Check if a specific hook is enabled
    #[must_use]
    pub fn is_hook_enabled(&self, hook_name: &str) -> bool {
        self.enabled && !self.disabled.contains(&hook_name.to_string())
    }
}
