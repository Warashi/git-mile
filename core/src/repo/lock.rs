use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// Access mode used when acquiring a repository lock.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LockMode {
    Read,
    Write,
}

/// Guard representing a held repository lock. The lock is released when dropped.
pub struct RepositoryLockGuard {
    file: File,
    path: PathBuf,
    mode: LockMode,
}

impl RepositoryLockGuard {
    fn new(file: File, path: PathBuf, mode: LockMode) -> Self {
        Self { file, path, mode }
    }

    fn unlock(&self) {
        if let Err(err) = self.file.unlock() {
            eprintln!(
                "warning: failed to release repository lock {:?} ({:?}): {}",
                self.path, self.mode, err
            );
        }
    }
}

impl Drop for RepositoryLockGuard {
    fn drop(&mut self) {
        self.unlock();
    }
}

/// Provides helper functions for acquiring repository-wide locks.
pub struct RepositoryLock;

impl RepositoryLock {
    /// Acquire a shared or exclusive lock for the repository. Blocks until the lock is available.
    pub fn acquire(repo_path: &Path, mode: LockMode) -> io::Result<RepositoryLockGuard> {
        let (file, path) = Self::open_lock_file(repo_path)?;
        match mode {
            LockMode::Read => fs2::FileExt::lock_shared(&file)?,
            LockMode::Write => fs2::FileExt::lock_exclusive(&file)?,
        }
        Ok(RepositoryLockGuard::new(file, path, mode))
    }

    /// Attempt to acquire the lock without blocking.
    pub fn try_acquire(repo_path: &Path, mode: LockMode) -> io::Result<RepositoryLockGuard> {
        let (file, path) = Self::open_lock_file(repo_path)?;
        let result = match mode {
            LockMode::Read => fs2::FileExt::try_lock_shared(&file),
            LockMode::Write => fs2::FileExt::try_lock_exclusive(&file),
        };

        result.map(|()| RepositoryLockGuard::new(file, path, mode))
    }

    fn open_lock_file(repo_path: &Path) -> io::Result<(File, PathBuf)> {
        let lock_dir = repo_path.join("git-mile");
        fs::create_dir_all(&lock_dir)?;
        let lock_path = lock_dir.join("lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)?;
        Ok((file, lock_path))
    }
}
