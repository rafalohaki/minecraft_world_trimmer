use thiserror::Error;

use std::fs::{File, OpenOptions};
use std::path::Path;

/// Guards against concurrent access to a world directory via the same
/// mechanism the game itself uses: an exclusive advisory lock on
/// `<world>/session.lock`. A running server holds this lock for its whole
/// lifetime, so acquiring it here proves no live process is writing the world.
///
/// The lock handle must be kept alive for as long as the world is processed.
#[must_use = "dropping SessionLock releases the exclusive lock"]
pub struct SessionLock {
    // Field order matters: the file is closed (releasing the lock) before the
    // path is dropped, mirroring vanilla's LevelStorageAccess teardown.
    _file: File,
    _world: std::path::PathBuf,
}

/// Acquires an exclusive lock on `<world_dir>/session.lock`, creating the file
/// if absent. Fails with [`SessionLockError::Held`] when another process holds
/// it — i.e. the world is currently open by a game/server.
pub fn acquire_session_lock(world_dir: &Path) -> Result<SessionLock, SessionLockError> {
    let lock_path = world_dir.join("session.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|e| SessionLockError::Open {
            path: lock_path.clone(),
            source: e,
        })?;

    match file.try_lock() {
        Ok(()) => Ok(SessionLock {
            _file: file,
            _world: world_dir.to_path_buf(),
        }),
        Err(std::fs::TryLockError::WouldBlock) => {
            Err(SessionLockError::Held(world_dir.to_path_buf()))
        }
        // Any other lock error (permissions, FS without locking): treat as
        // held — refusing to run is the safe direction.
        Err(_) => Err(SessionLockError::Held(world_dir.to_path_buf())),
    }
}

#[derive(Error, Debug)]
pub enum SessionLockError {
    /// Another process holds the session lock — refuse to touch the world.
    #[error(
        "the world `{0}` is locked by a running game/server (session.lock is held); \
         stop the server before running this tool"
    )]
    Held(std::path::PathBuf),
    #[error("cannot open session lock file `{path}`")]
    Open {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl std::fmt::Debug for SessionLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionLock")
            .field("world", &self._world)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_tmp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mwt_lock_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn lock_is_acquired_and_reentrant_within_process() {
        let dir = unique_tmp_dir("acquire");
        // First acquisition succeeds.
        let lock = acquire_session_lock(&dir).expect("lock must be acquired");
        // Same process re-opening the file: flock is per open-file-description,
        // so a second independent open would block — but dropping releases.
        drop(lock);
        let lock2 = acquire_session_lock(&dir).expect("lock must be reacquirable after release");
        drop(lock2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn second_independent_open_conflicts() {
        let dir = unique_tmp_dir("conflict");
        let _lock = acquire_session_lock(&dir).expect("first lock must succeed");
        // A separate open of the same lock file must be refused while held.
        assert!(matches!(
            acquire_session_lock(&dir),
            Err(SessionLockError::Held(_))
        ));
        std::fs::remove_dir_all(&dir).ok();
    }
}
