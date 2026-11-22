//! The session-name registry (design C16, amended): which project owns which session key on this
//! runner. It is kept in `sessions/registry.json` under the state directory (mode 0600).
//!
//! - A project keeps every key it has used: a renamed project's old key stays reserved to it
//!   while a session for it may exist, so another project cannot take the name over and be pushed
//!   into, or kill, the old session. A key is released only by [`SessionRegistry::release`], which
//!   the `sessions` command calls for a key whose session is gone.
//! - A key is bound only after every check of a delivery has passed; a refused delivery never
//!   binds or changes anything ([`SessionRegistry::check`] is read-only).
//! - A registry file that cannot be read or parsed is an error, not an empty registry: the
//!   protection would be lost silently. The file is left as it is for the operator.
//! - Writes are serialised, the newest state is what is written, and the file is synced, renamed
//!   into place and the directory synced.

use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::checks::valid_project_key;

#[derive(Serialize, Deserialize, Default)]
struct Stored {
    version: u32,
    /// Session key to the id of the project that owns it.
    keys: BTreeMap<String, String>,
}

/// The registry.
#[derive(Debug, Default)]
pub struct SessionRegistry {
    file: Option<PathBuf>,
    keys: Mutex<BTreeMap<String, String>>,
    /// Serialises writers so an older snapshot is never renamed over a newer one.
    writer: Mutex<()>,
}

/// A key is owned by another project.
#[derive(Debug, PartialEq, Eq)]
pub struct Collision;

impl SessionRegistry {
    /// Opens the registry at `<state_dir>/sessions/registry.json`, or an in-memory one without a
    /// state directory.
    ///
    /// # Errors
    ///
    /// A message when the file exists but cannot be read or parsed.
    pub fn open(state_dir: Option<&Path>) -> Result<Self, String> {
        let file = state_dir.map(|dir| dir.join("sessions").join("registry.json"));
        let mut keys = BTreeMap::new();
        if let Some(file) = &file {
            match std::fs::read(file) {
                Ok(bytes) => {
                    let stored: Stored = serde_json::from_slice(&bytes).map_err(|_| {
                        format!(
                            "the session registry {} is not valid JSON of the expected shape; fix or remove it",
                            file.display()
                        )
                    })?;
                    if stored.version != 1 {
                        return Err(format!(
                            "the session registry {} has an unknown version",
                            file.display()
                        ));
                    }
                    for (key, project) in stored.keys {
                        if valid_project_key(&key) && uuid::Uuid::parse_str(&project).is_ok() {
                            keys.insert(key, project);
                        } else {
                            tracing::warn!("an invalid entry of the session registry was dropped");
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(format!(
                        "the session registry {} cannot be read: {}",
                        file.display(),
                        e.kind()
                    ));
                }
            }
        }
        Ok(Self {
            file,
            keys: Mutex::new(keys),
            writer: Mutex::new(()),
        })
    }

    /// Whether `project_id` may use `key`. Read-only.
    ///
    /// # Errors
    ///
    /// [`Collision`] when another project owns the key.
    pub fn check(&self, project_id: &str, key: &str) -> Result<(), Collision> {
        match self.keys.lock().get(key) {
            Some(owner) if owner != project_id => Err(Collision),
            _ => Ok(()),
        }
    }

    /// Records that `project_id` owns `key`: called once every check of a delivery has passed.
    ///
    /// # Errors
    ///
    /// [`Collision`] when another project owns the key.
    pub fn bind(&self, project_id: &str, key: &str) -> Result<(), Collision> {
        {
            let mut keys = self.keys.lock();
            match keys.get(key) {
                Some(owner) if owner != project_id => return Err(Collision),
                Some(_) => return Ok(()),
                None => {
                    keys.insert(key.to_string(), project_id.to_string());
                }
            }
        }
        self.save();
        Ok(())
    }

    /// Releases `key`. Only for a key whose session is verified gone (the `sessions` command).
    /// Returns whether it was bound.
    pub fn release(&self, key: &str) -> bool {
        let removed = self.keys.lock().remove(key).is_some();
        if removed {
            self.save();
        }
        removed
    }

    /// The keys and their owners, sorted.
    pub fn entries(&self) -> Vec<(String, String)> {
        self.keys.lock().iter().map(|(k, p)| (k.clone(), p.clone())).collect()
    }

    /// The owner of `key`.
    pub fn owner(&self, key: &str) -> Option<String> {
        self.keys.lock().get(key).cloned()
    }

    fn save(&self) {
        let Some(file) = &self.file else { return };
        let _writing = self.writer.lock();
        let snapshot = self.keys.lock().clone();
        if let Err(e) = persist(file, &snapshot) {
            tracing::error!("could not save the session registry: {e}");
        }
    }
}

fn persist(file: &Path, keys: &BTreeMap<String, String>) -> Result<(), String> {
    let dir = file.parent().ok_or("no directory")?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|e| e.kind().to_string())?;
    let temp = dir.join(format!(".registry.{}.tmp", uuid::Uuid::new_v4().simple()));
    let result = (|| {
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)
            .map_err(|e| e.kind().to_string())?;
        let stored = Stored {
            version: 1,
            keys: keys.clone(),
        };
        out.write_all(&serde_json::to_vec(&stored).map_err(|e| e.to_string())?)
            .map_err(|e| e.kind().to_string())?;
        out.sync_all().map_err(|e| e.kind().to_string())?;
        std::fs::rename(&temp, file).map_err(|e| e.kind().to_string())?;
        // The rename itself must survive a crash.
        std::fs::File::open(dir)
            .and_then(|d| d.sync_all())
            .map_err(|e| e.kind().to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}
