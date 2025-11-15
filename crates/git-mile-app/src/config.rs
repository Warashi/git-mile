use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, bail, Context, Result};
use git2::Repository;
pub use git_mile_core::StateKind;
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
        self.workflow.ensure_unique_states()?;
        self.workflow.ensure_valid_default()
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
#[derive(Debug, Clone, Deserialize)]
pub struct WorkflowConfig {
    #[serde(default)]
    states: Vec<WorkflowState>,
    #[serde(default)]
    default_state: Option<String>,
}

impl Default for WorkflowConfig {
    fn default() -> Self {
        Self {
            states: Self::builtin_states(),
            default_state: Some("state/todo".into()),
        }
    }
}

impl WorkflowConfig {
    /// Configuration without workflow restrictions (used mainly in tests).
    pub const fn unrestricted() -> Self {
        Self {
            states: Vec::new(),
            default_state: None,
        }
    }

    fn builtin_states() -> Vec<WorkflowState> {
        vec![
            WorkflowState {
                value: "state/todo".into(),
                label: Some("Todo".into()),
                kind: Some(StateKind::Todo),
            },
            WorkflowState {
                value: "state/in-progress".into(),
                label: Some("In Progress".into()),
                kind: Some(StateKind::InProgress),
            },
            WorkflowState {
                value: "state/done".into(),
                label: Some("Done".into()),
                kind: Some(StateKind::Done),
            },
        ]
    }

    /// Construct a workflow configuration from explicit states.
    pub const fn from_states(states: Vec<WorkflowState>) -> Self {
        Self {
            states,
            default_state: None,
        }
    }

    /// Construct workflow configuration with explicit default state.
    pub fn from_states_with_default(states: Vec<WorkflowState>, default_state: Option<&str>) -> Self {
        Self {
            states,
            default_state: default_state.map(str::to_owned),
        }
    }

    /// Returns true when states are restricted to a configured set.
    pub const fn is_restricted(&self) -> bool {
        !self.states.is_empty()
    }

    /// Iterate over allowed workflow states (if any).
    pub fn states(&self) -> &[WorkflowState] {
        &self.states
    }

    /// Retrieve configured default state (if any).
    pub fn default_state(&self) -> Option<&str> {
        self.default_state.as_deref()
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

    fn ensure_valid_default(&self) -> Result<()> {
        let Some(default) = self.default_state() else {
            return Ok(());
        };
        if default.trim().is_empty() {
            bail!("default workflow state must not be empty");
        }
        if self.is_restricted() && self.find_state(default).is_none() {
            bail!("default workflow state '{default}' is not defined in workflow configuration");
        }
        Ok(())
    }

    /// Resolve the configured state kind (if any) for the provided workflow state.
    pub fn resolve_state_kind(&self, value: Option<&str>) -> Option<StateKind> {
        value.and_then(|state| self.find_state(state).and_then(WorkflowState::kind))
    }
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
    fn missing_config_returns_builtin_workflow() -> Result<()> {
        let dir = tempdir()?;
        let cfg = ProjectConfig::from_workdir(dir.path())?;
        assert!(cfg.workflow.is_restricted());
        assert_eq!(cfg.workflow.states().len(), 3);
        assert_eq!(cfg.workflow.default_state(), Some("state/todo"));
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
            "[workflow]\nstates = [\n  {{ value = \"state/todo\", label = \"To Do\" }},\n  {{ value = \"state/done\" }}\n]\ndefault_state = \"state/todo\""
        )?;

        let cfg = ProjectConfig::from_workdir(dir.path())?;
        assert!(cfg.workflow.is_restricted());
        assert_eq!(cfg.workflow.states().len(), 2);
        cfg.workflow.validate_state(Some("state/todo"))?;
        assert!(cfg.workflow.validate_state(Some("state/unknown")).is_err());
        assert_eq!(cfg.workflow.default_state(), Some("state/todo"));
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

    #[test]
    fn default_state_must_be_defined_when_restricted() -> Result<()> {
        let dir = tempdir()?;
        let cfg_dir = dir.path().join(CONFIG_DIR);
        fs::create_dir_all(&cfg_dir)?;
        let mut file = fs::File::create(cfg_dir.join(CONFIG_FILE))?;
        writeln!(
            file,
            "[workflow]\nstates = [\n  {{ value = \"state/todo\" }}\n]\ndefault_state = \"state/done\""
        )?;

        let Err(err) = ProjectConfig::from_workdir(dir.path()) else {
            panic!("unknown default state should error");
        };
        assert!(err.to_string().contains("default workflow state 'state/done'"));
        Ok(())
    }

    #[test]
    fn default_state_must_not_be_empty() -> Result<()> {
        let dir = tempdir()?;
        let cfg_dir = dir.path().join(CONFIG_DIR);
        fs::create_dir_all(&cfg_dir)?;
        let mut file = fs::File::create(cfg_dir.join(CONFIG_FILE))?;
        writeln!(file, "[workflow]\ndefault_state = \"\"")?;

        let Err(err) = ProjectConfig::from_workdir(dir.path()) else {
            panic!("empty default state should error");
        };
        assert!(err
            .to_string()
            .contains("default workflow state must not be empty"));
        Ok(())
    }

    #[test]
    fn unrestricted_workflow_has_no_states_or_default() {
        let workflow = WorkflowConfig::unrestricted();
        assert!(!workflow.is_restricted());
        assert!(workflow.default_state().is_none());
    }

    #[test]
    fn display_label_prefers_label_and_defaults_to_unset() {
        let workflow = WorkflowConfig::from_states_with_default(
            vec![
                WorkflowState {
                    value: "state/todo".into(),
                    label: Some("Todo".into()),
                    kind: Some(StateKind::Todo),
                },
                WorkflowState {
                    value: "state/custom".into(),
                    label: None,
                    kind: None,
                },
            ],
            Some("state/todo"),
        );

        assert_eq!(workflow.display_label(Some("state/todo")), "Todo");
        assert_eq!(workflow.display_label(Some("state/custom")), "state/custom");
        assert_eq!(workflow.display_label(None), "未設定");
    }

    #[test]
    fn state_hint_lists_labels_and_resolves_state_kind() {
        let workflow = WorkflowConfig::from_states(vec![WorkflowState {
            value: "state/in-progress".into(),
            label: Some("Doing".into()),
            kind: Some(StateKind::InProgress),
        }]);

        let Some(hint) = workflow.state_hint() else {
            panic!("state hint");
        };
        assert_eq!(hint, "state/in-progress (Doing)");
        assert_eq!(
            workflow.resolve_state_kind(Some("state/in-progress")),
            Some(StateKind::InProgress)
        );
        assert!(workflow.resolve_state_kind(Some("state/unknown")).is_none());
        assert!(workflow.resolve_state_kind(None).is_none());
    }
}
