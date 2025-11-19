//! Helper utilities for resolving [`Actor`](git_mile_core::event::Actor) from
//! environment variables or Git configuration.

use std::env;
use std::path::Path;

use anyhow::{Context, Result};
use git_mile_core::event::Actor;
use git2::Repository;

/// Environment variable checked first for actor names.
pub const ENV_ACTOR_NAME: &str = "GIT_MILE_ACTOR_NAME";
/// Environment variable checked first for actor emails.
pub const ENV_ACTOR_EMAIL: &str = "GIT_MILE_ACTOR_EMAIL";
/// Fallback default display name when no data can be resolved.
pub const DEFAULT_ACTOR_NAME: &str = "git-mile";
/// Fallback default email when no data can be resolved.
pub const DEFAULT_ACTOR_EMAIL: &str = "git-mile@example.invalid";

const FALLBACK_AUTHOR_NAME_ENV: &str = "GIT_AUTHOR_NAME";
const FALLBACK_AUTHOR_EMAIL_ENV: &str = "GIT_AUTHOR_EMAIL";
const USER_NAME_ENV: &str = "USER";

/// Resolve an actor purely from environment variables.
///
/// Checks the git-mile specific variables first (`GIT_MILE_ACTOR_NAME`,
/// `GIT_MILE_ACTOR_EMAIL`) and then falls back to the conventional `GIT_AUTHOR_*`
/// variables.
///
/// # Errors
/// Returns an error if either the name or email cannot be resolved from the environment.
pub fn actor_from_env() -> Result<Actor> {
    let mut fetch = |key: &'static str| env::var(key).ok();
    actor_from_env_with(&mut fetch)
}

/// Resolve an actor from the Git configuration reachable from `repo_hint`.
///
/// # Errors
/// Returns an error when the repository cannot be discovered or the `user.name`/`user.email`
/// values are missing.
pub fn actor_from_git_config<P: AsRef<Path>>(repo_hint: P) -> Result<Actor> {
    let repo = Repository::discover(repo_hint)?;
    let config = repo.config()?;
    let name = config
        .get_string("user.name")
        .context("user.name not configured in Git")?;
    let email = config
        .get_string("user.email")
        .context("user.email not configured in Git")?;
    Ok(Actor { name, email })
}

/// Resolve an actor using the standard fallback order (env → git config → defaults).
pub fn default_actor<P: AsRef<Path>>(repo_hint: P) -> Actor {
    let mut fetch = |key: &'static str| env::var(key).ok();
    default_actor_with_env(repo_hint, &mut fetch)
}

fn default_actor_with_env<P: AsRef<Path>>(
    repo_hint: P,
    fetch: &mut impl FnMut(&'static str) -> Option<String>,
) -> Actor {
    actor_from_env_with(fetch)
        .or_else(|_| actor_from_git_config(repo_hint))
        .unwrap_or_else(|_| Actor {
            name: DEFAULT_ACTOR_NAME.to_owned(),
            email: DEFAULT_ACTOR_EMAIL.to_owned(),
        })
}

/// Build an actor from optional CLI/MCP parameters with sane fallbacks.
///
/// Whenever one of the fields is missing, the default actor is used and then
/// selectively overridden with the provided value(s). This allows callers to
/// override just the name or email instead of both.
pub fn actor_from_params_or_default<P: AsRef<Path>>(
    name: Option<&str>,
    email: Option<&str>,
    repo_hint: P,
) -> Actor {
    if name.is_none() && email.is_none() {
        return default_actor(repo_hint);
    }

    let mut actor = default_actor(repo_hint);
    if let Some(value) = name {
        actor.name.clear();
        actor.name.push_str(value);
    }
    if let Some(value) = email {
        actor.email.clear();
        actor.email.push_str(value);
    }
    actor
}

fn env_value_with(
    candidates: &[&'static str],
    fetch: &mut impl FnMut(&'static str) -> Option<String>,
) -> Option<String> {
    candidates.iter().find_map(|key| {
        fetch(key).and_then(|value| {
            if value.trim().is_empty() {
                None
            } else {
                Some(value)
            }
        })
    })
}

fn actor_from_env_with(fetch: &mut impl FnMut(&'static str) -> Option<String>) -> Result<Actor> {
    let name = env_value_with(&[ENV_ACTOR_NAME, FALLBACK_AUTHOR_NAME_ENV, USER_NAME_ENV], fetch)
        .context("environment does not include actor name")?;
    let email = env_value_with(&[ENV_ACTOR_EMAIL, FALLBACK_AUTHOR_EMAIL_ENV], fetch)
        .context("environment does not include actor email")?;
    Ok(Actor { name, email })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn default_actor_prefers_explicit_environment() -> Result<()> {
        let mut fetch = |key: &'static str| match key {
            ENV_ACTOR_NAME => Some("env-name".into()),
            ENV_ACTOR_EMAIL => Some("env@example.invalid".into()),
            _ => None,
        };
        let actor = actor_from_env_with(&mut fetch)?;
        assert_eq!(actor.name, "env-name");
        assert_eq!(actor.email, "env@example.invalid");
        Ok(())
    }

    #[test]
    fn default_actor_falls_back_to_git_config() -> Result<()> {
        let temp = TempDir::new()?;
        let repo = Repository::init(temp.path())?;
        {
            let mut config = repo.config()?;
            config.set_str("user.name", "git-name")?;
            config.set_str("user.email", "git@example.invalid")?;
        }

        let mut fetch = |_: &'static str| None;
        let actor = default_actor_with_env(temp.path(), &mut fetch);
        assert_eq!(actor.name, "git-name");
        assert_eq!(actor.email, "git@example.invalid");
        Ok(())
    }

    #[test]
    fn params_override_defaults_selectively() -> Result<()> {
        let temp = TempDir::new()?;
        let repo = Repository::init(temp.path())?;
        {
            let mut config = repo.config()?;
            config.set_str("user.name", "git-name")?;
            config.set_str("user.email", "git@example.invalid")?;
        }

        let actor = actor_from_params_or_default(Some("cli-name"), None, temp.path());
        assert_eq!(actor.name, "cli-name");
        assert_eq!(actor.email, "git@example.invalid");

        let actor = actor_from_params_or_default(None, Some("cli@example.com"), temp.path());
        assert_eq!(actor.name, "git-name");
        assert_eq!(actor.email, "cli@example.com");

        let actor = actor_from_params_or_default(Some("cli-name"), Some("cli@example.com"), temp.path());
        assert_eq!(actor.name, "cli-name");
        assert_eq!(actor.email, "cli@example.com");
        Ok(())
    }
}
