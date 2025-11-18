//! Hook execution logic

use crate::{HookContext, HookError, HookKind, HookResult, HooksConfig, Result};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Executor for running hook scripts
#[derive(Debug)]
pub struct HookExecutor {
    config: HooksConfig,
    base_dir: PathBuf,
}

impl HookExecutor {
    /// Create a new hook executor
    ///
    /// # Arguments
    ///
    /// * `config` - Hook configuration
    /// * `base_dir` - Base directory (usually .git-mile directory)
    #[must_use]
    pub const fn new(config: HooksConfig, base_dir: PathBuf) -> Self {
        Self { config, base_dir }
    }

    /// Execute a hook script
    ///
    /// # Arguments
    ///
    /// * `kind` - The kind of hook to execute
    /// * `context` - Context to pass to the hook
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Hook execution fails
    /// - Hook times out
    /// - I/O error occurs
    /// - JSON serialization fails
    pub fn execute(&self, kind: HookKind, context: &HookContext) -> Result<HookResult> {
        let hook_name = kind.script_name();

        // Check if hooks are enabled and this specific hook is enabled
        if !self.config.enabled || !self.config.is_hook_enabled(hook_name) {
            return Ok(HookResult {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
                modified_event: None,
            });
        }

        // Find the hook script
        let script_path = self.find_hook_script(hook_name)?;

        // Serialize context to JSON
        let input_json = serde_json::to_string(context)?;

        // Execute the script with timeout
        self.execute_script(&script_path, &input_json)
    }

    /// Find the hook script file
    fn find_hook_script(&self, hook_name: &str) -> Result<PathBuf> {
        let hooks_dir = self.base_dir.join(&self.config.hooks_dir);
        let script_path = hooks_dir.join(hook_name);

        // Check if the script exists and is executable
        if script_path.exists() {
            Ok(script_path)
        } else {
            Err(HookError::NotFound(format!(
                "{hook_name} in {}",
                hooks_dir.display()
            )))
        }
    }

    /// Execute a script with timeout
    fn execute_script(&self, script_path: &Path, input_json: &str) -> Result<HookResult> {
        let mut child = Command::new(script_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        // Write input to stdin
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(input_json.as_bytes())?;
        }

        // Wait for the process with timeout
        let timeout_duration = Duration::from_secs(self.config.timeout);
        let output = wait_with_timeout(&mut child, timeout_duration)?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let exit_code = output.status.code().unwrap_or(-1);

        // Try to parse modified event from stdout
        let modified_event = if stdout.is_empty() {
            None
        } else {
            serde_json::from_str(&stdout).ok()
        };

        Ok(HookResult {
            exit_code,
            stdout,
            stderr,
            modified_event,
        })
    }
}

/// Wait for a child process with timeout
///
/// # Errors
///
/// Returns `HookError::Timeout` if the process doesn't complete within the timeout
fn wait_with_timeout(child: &mut std::process::Child, timeout: Duration) -> Result<std::process::Output> {
    // For simplicity, we'll use a polling approach
    // In a real implementation, you might want to use tokio or a platform-specific API
    let start = std::time::Instant::now();
    let poll_interval = Duration::from_millis(100);

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                // Process has finished - collect output
                let stdout = child.stdout.take().map_or(Vec::new(), |mut stdout| {
                    let mut buf = Vec::new();
                    let _ = std::io::Read::read_to_end(&mut stdout, &mut buf);
                    buf
                });

                let stderr = child.stderr.take().map_or(Vec::new(), |mut stderr| {
                    let mut buf = Vec::new();
                    let _ = std::io::Read::read_to_end(&mut stderr, &mut buf);
                    buf
                });

                return Ok(std::process::Output {
                    status,
                    stdout,
                    stderr,
                });
            }
            Ok(None) => {
                // Process is still running
                if start.elapsed() >= timeout {
                    // Timeout - kill the process
                    let _ = child.kill();
                    return Err(HookError::Timeout(timeout.as_secs()));
                }
                std::thread::sleep(poll_interval);
            }
            Err(e) => {
                return Err(HookError::ExecutionFailed(e.to_string()));
            }
        }
    }
}
