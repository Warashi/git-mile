use anyhow::Result;
use git_mile_store_git::GitStore;

pub fn run_push(store: &GitStore, remote: &str, force: bool) -> Result<()> {
    store.push_refs(remote, force)?;
    println!("Successfully pushed task refs to remote '{remote}'");
    Ok(())
}

pub fn run_pull(store: &GitStore, remote: &str) -> Result<()> {
    store.pull_refs(remote)?;
    println!("Successfully pulled task refs from remote '{remote}'");
    Ok(())
}
