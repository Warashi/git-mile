use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, bail, Context, Result};
use git2::Repository;
use serde::Deserialize;

const CONFIG_DIR: &str = ".git-mile";
const CONFIG_FILE: &str = "config.toml";

/// Top-level project configuration loaded from `.git-mile/config.toml`.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ProjectConfig {
    #[serde(default)]
    pub workflow: WorkflowConfig,
}

impl ProjectConfig {
    /// Load configuration by discovering the nearest Git repository from `cwd_or_repo`.
    pub fn load(cwd_or_repo: impl AsRef<Path>) -> Result<Self> {
        let repo = Repository::discover(cwd_or_repo)?;
        Self::from_repository(&repo)
    }

    /// Load configuration using an opened repository reference.
    pub fn from_repository(repo: &Repository) -> Result<Self> {
        let workdir = repo_workdir(repo)?;
        Self::from_workdir(workdir)
    }

    /// Load configuration from a known working tree directory.
    pub fn from_workdir(workdir: impl AsRef<Path>) -> Result<Self> {
        let config_path = workdir.as_ref().join(CONFIG_DIR).join(CONFIG_FILE);
        if !config_path.exists() {
            return Ok(Self::default());
        }

        let contents = fs::read_to_string(&config_path)
            .with_context(|| format!("failed to read {}", config_path.display()))?;
        let config: Self = toml::from_str(&contents)
            .with_context(|| format!("failed to parse {}", config_path.display()))?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        self.workflow.ensure_unique_states()
    }
}

fn repo_workdir(repo: &Repository) -> Result<PathBuf> {
    if let Some(workdir) = repo.workdir() {
        return Ok(workdir.to_path_buf());
    }
    // Bare repositories don't have a working tree. Fallback to the repository path itself.
    repo.path()
        .parent()
        .map(Path::to_path_buf)
        .or_else(|| Some(repo.path().to_path_buf()))
        .ok_or_else(|| anyhow!("failed to resolve repository root"))
}

/// Workflow configuration block.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct WorkflowConfig {
    #[serde(default)]
    states: Vec<WorkflowState>,
}

impl WorkflowConfig {
    /// Construct a workflow configuration from explicit states.
    #[cfg(test)]
    pub const fn from_states(states: Vec<WorkflowState>) -> Self {
        Self { states }
    }

    /// Returns true when states are restricted to a configured set.
    pub const fn is_restricted(&self) -> bool {
        !self.states.is_empty()
    }

    /// Iterate over allowed workflow states (if any).
    pub(crate) fn states(&self) -> &[WorkflowState] {
        &self.states
    }

    /// Find a workflow state by its value.
    pub fn find_state(&self, value: &str) -> Option<&WorkflowState> {
        self.states.iter().find(|state| state.value() == value)
    }

    /// Get display label for a state value, using label if available, otherwise the value itself.
    pub fn display_label<'a>(&'a self, value: Option<&'a str>) -> &'a str {
        value
            .and_then(|v| self.find_state(v).and_then(|state| state.label()).or(Some(v)))
            .unwrap_or("未設定")
    }

    /// Get state marker based on state kind.
    pub fn state_marker(&self, value: Option<&str>) -> &'static str {
        value
            .and_then(|v| self.find_state(v))
            .and_then(WorkflowState::kind)
            .map_or("", |kind| match kind {
                StateKind::Done => " ✓",
                StateKind::InProgress => " →",
                StateKind::Blocked => " ⊗",
                StateKind::Todo => " □",
                StateKind::Backlog => " ◇",
            })
    }

    /// Validate that the provided state (if any) is part of the configured set.
    pub fn validate_state(&self, candidate: Option<&str>) -> Result<()> {
        let Some(value) = candidate else {
            return Ok(());
        };
        if !self.is_restricted() {
            return Ok(());
        }
        if self.states.iter().any(|state| state.value() == value) {
            return Ok(());
        }
        let hint = self
            .state_hint()
            .map(|hint| format!(" Allowed values: {hint}."))
            .unwrap_or_default();
        bail!("state '{value}' is not defined in workflow configuration.{hint}");
    }

    /// Human-readable hint string for editor templates / error messages.
    pub fn state_hint(&self) -> Option<String> {
        if self.states.is_empty() {
            None
        } else {
            Some(
                self.states
                    .iter()
                    .map(WorkflowState::describe)
                    .collect::<Vec<_>>()
                    .join(", "),
            )
        }
    }

    fn ensure_unique_states(&self) -> Result<()> {
        let mut seen = HashSet::new();
        for state in &self.states {
            if !seen.insert(state.value()) {
                bail!("duplicate workflow state detected: {}", state.value());
            }
        }
        Ok(())
    }
}

/// Classification of workflow state behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateKind {
    /// Task is completed.
    Done,
    /// Task is actively being worked on.
    InProgress,
    /// Task is blocked or waiting.
    Blocked,
    /// Task is ready to be worked on (prioritized).
    Todo,
    /// Task is in backlog (not yet prioritized).
    Backlog,
}

/// Individual workflow state definition.
#[derive(Debug, Clone, Deserialize)]
pub struct WorkflowState {
    value: String,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    kind: Option<StateKind>,
}

impl WorkflowState {
    /// Create a workflow state with the given wire value.
    #[cfg(test)]
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: None,
            kind: None,
        }
    }

    /// Get the state value.
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Optional human-friendly label.
    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    /// Optional state classification.
    pub const fn kind(&self) -> Option<StateKind> {
        self.kind
    }

    fn describe(&self) -> String {
        match self.label() {
            Some(label) if label != self.value => format!("{} ({label})", self.value),
            _ => self.value.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn missing_config_returns_default() -> Result<()> {
        let dir = tempdir()?;
        let cfg = ProjectConfig::from_workdir(dir.path())?;
        assert!(!cfg.workflow.is_restricted());
        Ok(())
    }

    #[test]
    fn load_config_with_states() -> Result<()> {
        let dir = tempdir()?;
        let cfg_dir = dir.path().join(CONFIG_DIR);
        fs::create_dir_all(&cfg_dir)?;
        let mut file = fs::File::create(cfg_dir.join(CONFIG_FILE))?;
        writeln!(
            file,
            "[workflow]\nstates = [\n  {{ value = \"state/todo\", label = \"To Do\" }},\n  {{ value = \"state/done\" }}\n]"
        )?;

        let cfg = ProjectConfig::from_workdir(dir.path())?;
        assert!(cfg.workflow.is_restricted());
        assert_eq!(cfg.workflow.states().len(), 2);
        cfg.workflow.validate_state(Some("state/todo"))?;
        assert!(cfg.workflow.validate_state(Some("state/unknown")).is_err());
        Ok(())
    }

    #[test]
    fn duplicate_states_are_rejected() -> Result<()> {
        let dir = tempdir()?;
        let cfg_dir = dir.path().join(CONFIG_DIR);
        fs::create_dir_all(&cfg_dir)?;
        let mut file = fs::File::create(cfg_dir.join(CONFIG_FILE))?;
        writeln!(
            file,
            "[workflow]\nstates = [\n  {{ value = \"state/todo\" }},\n  {{ value = \"state/todo\" }}\n]"
        )?;

        let Err(err) = ProjectConfig::from_workdir(dir.path()) else {
            panic!("duplicate workflow state should error");
        };
        assert!(err.to_string().contains("duplicate workflow state"));
        Ok(())
    }
}
