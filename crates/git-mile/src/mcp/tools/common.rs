//! Shared helpers for MCP tool implementations.

use git_mile_store_git::GitStore;
use rmcp::ErrorData as McpError;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Run a blocking store operation on a clone of the shared [`GitStore`].
pub async fn with_store<F, R>(store: Arc<Mutex<GitStore>>, action: F) -> Result<R, McpError>
where
    F: FnOnce(GitStore) -> Result<R, McpError> + Send + 'static,
    R: Send + 'static,
{
    let cloned = {
        let guard = store.lock().await;
        guard.clone()
    };
    tokio::task::spawn_blocking(move || action(cloned))
        .await
        .map_err(|e| McpError::internal_error(format!("Task join error: {e}"), None))?
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn runs_blocking_action_without_holding_mutex() {
        let dir = tempdir().expect("create temp dir");
        git2::Repository::init(dir.path()).expect("init git repo");
        let store = GitStore::open(dir.path()).expect("open store");
        let shared = Arc::new(Mutex::new(store));

        let (first, second) = tokio::join!(
            with_store(shared.clone(), |_store| Ok::<_, McpError>("first")),
            with_store(shared.clone(), |_store| Ok::<_, McpError>("second")),
        );

        assert_eq!(first.unwrap(), "first");
        assert_eq!(second.unwrap(), "second");
    }
}
