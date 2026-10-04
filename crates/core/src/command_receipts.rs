//! Durable receipts for commands a remote caller may retry after losing a reply.
//! The caller must hold its project's coordinator lock while executing commands.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;

static COMMAND_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Debug, thiserror::Error)]
pub enum CommandReceiptError {
    #[error("commandId must contain 1..200 ASCII letters, digits, underscores or hyphens")]
    InvalidId,
    #[error("commandId was already used for different arguments or a different command")]
    Conflict,
    #[error("the previous command stopped before its outcome was saved; inspect the job before issuing a new commandId")]
    Incomplete,
    #[error("command receipt could not be saved or read: {0}")]
    Storage(String),
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Receipt<T> {
    version: u32,
    fingerprint: String,
    outcome: ReceiptOutcome<T>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum ReceiptOutcome<T> {
    Pending,
    Completed { value: T },
}

pub struct CommandReceiptStore {
    dir: PathBuf,
}

impl CommandReceiptStore {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            dir: root.as_ref().join(".zest").join("command-receipts"),
        }
    }

    /// Save intent before invoking the action, then save its exact response.
    /// A pending receipt after a crash never silently re-executes the action.
    /// Arguments are hashed, not stored. Outcomes must contain no credentials.
    pub fn execute<T, F>(
        &self,
        command_id: &str,
        command: &str,
        arguments: &Value,
        action: F,
    ) -> Result<T, CommandReceiptError>
    where
        T: Serialize + DeserializeOwned,
        F: FnOnce() -> T,
    {
        if command_id.is_empty()
            || command_id.len() > 200
            || !command_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        {
            return Err(CommandReceiptError::InvalidId);
        }
        let _guard = COMMAND_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .map_err(|_| CommandReceiptError::Storage("command lock is unavailable".into()))?;
        let mut arguments = arguments.clone();
        arguments.sort_all_objects();
        let encoded = serde_json::to_vec(&arguments)
            .map_err(|error| CommandReceiptError::Storage(error.to_string()))?;
        let mut hasher = blake3::Hasher::new();
        hasher.update(command.as_bytes());
        hasher.update(&[0]);
        hasher.update(&encoded);
        let fingerprint = hasher.finalize().to_hex().to_string();
        let path = self.path_for(command_id);
        match std::fs::read(&path) {
            Ok(bytes) => {
                let stored: Receipt<T> = serde_json::from_slice(&bytes)
                    .map_err(|error| CommandReceiptError::Storage(error.to_string()))?;
                if stored.version != 1 {
                    return Err(CommandReceiptError::Storage(
                        "unsupported receipt version".into(),
                    ));
                }
                if stored.fingerprint != fingerprint {
                    return Err(CommandReceiptError::Conflict);
                }
                return match stored.outcome {
                    ReceiptOutcome::Pending => Err(CommandReceiptError::Incomplete),
                    ReceiptOutcome::Completed { value } => Ok(value),
                };
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(CommandReceiptError::Storage(error.to_string())),
        }
        std::fs::create_dir_all(&self.dir)
            .map_err(|error| CommandReceiptError::Storage(error.to_string()))?;
        // The workbench can serve any repository, not only Zest's own checkout.
        // Keep receipts untracked even when that project has no ignore rules.
        crate::fsutil::atomic_write(&self.dir.join(".gitignore"), b"*\n")
            .map_err(|error| CommandReceiptError::Storage(error.to_string()))?;
        let mut receipt = Receipt {
            version: 1,
            fingerprint,
            outcome: ReceiptOutcome::<T>::Pending,
        };
        crate::fsutil::atomic_write_json(&path, &receipt)
            .map_err(|error| CommandReceiptError::Storage(error.to_string()))?;
        receipt.outcome = ReceiptOutcome::Completed { value: action() };
        crate::fsutil::atomic_write_json(&path, &receipt)
            .map_err(|error| CommandReceiptError::Storage(error.to_string()))?;
        match receipt.outcome {
            ReceiptOutcome::Completed { value } => Ok(value),
            ReceiptOutcome::Pending => unreachable!("the action returned an outcome"),
        }
    }

    fn path_for(&self, command_id: &str) -> PathBuf {
        // Preserve case-sensitive identities on case-insensitive filesystems,
        // and keep Windows device names out of filenames.
        self.dir.join(format!(
            "{}.json",
            blake3::hash(command_id.as_bytes()).to_hex()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn retry_after_reopening_returns_the_original_outcome() {
        let temp = tempfile::tempdir().unwrap();
        let args = json!({"jobId": "job-1", "objective": "private prompt"});
        let result = CommandReceiptStore::new(temp.path())
            .execute("cmd-1", "approve", &args, || json!({"status": "queued"}))
            .unwrap();
        let retry = CommandReceiptStore::new(temp.path())
            .execute::<Value, _>("cmd-1", "approve", &args, || {
                panic!("retry must not execute")
            })
            .unwrap();
        assert_eq!(result, retry);
        let bytes =
            std::fs::read_to_string(CommandReceiptStore::new(temp.path()).path_for("cmd-1"))
                .unwrap();
        assert!(!bytes.contains("private prompt"));
        let conflict = CommandReceiptStore::new(temp.path()).execute::<Value, _>(
            "cmd-1",
            "cancel",
            &args,
            || panic!("conflict must not execute"),
        );
        assert!(matches!(conflict, Err(CommandReceiptError::Conflict)));
    }

    #[test]
    fn concurrent_retries_execute_once() {
        let temp = tempfile::tempdir().unwrap();
        let calls = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..4)
                .map(|_| {
                    scope.spawn(|| {
                        CommandReceiptStore::new(temp.path())
                            .execute("cmd-1", "retry", &json!({}), || {
                                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                                json!({"status": "awaiting_approval"})
                            })
                            .unwrap()
                    })
                })
                .collect();
            for handle in handles {
                assert_eq!(handle.join().unwrap()["status"], "awaiting_approval");
            }
        });
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn unfinished_or_corrupt_receipts_never_execute_again() {
        let temp = tempfile::tempdir().unwrap();
        let store = CommandReceiptStore::new(temp.path());
        store
            .execute("cmd-1", "approve", &json!({}), || json!({}))
            .unwrap();
        let path = store.path_for("cmd-1");
        let mut receipt: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        receipt["outcome"] = json!({"state": "pending"});
        crate::fsutil::atomic_write_json(&path, &receipt).unwrap();
        assert!(matches!(
            store.execute::<Value, _>("cmd-1", "approve", &json!({}), || panic!("pending command")),
            Err(CommandReceiptError::Incomplete)
        ));
        std::fs::write(&path, "corrupt").unwrap();
        assert!(matches!(
            store.execute::<Value, _>("cmd-1", "approve", &json!({}), || panic!("corrupt command")),
            Err(CommandReceiptError::Storage(_))
        ));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "corrupt");
    }

    #[test]
    fn invalid_id_cannot_escape_the_receipt_directory() {
        let temp = tempfile::tempdir().unwrap();
        for id in ["", "../escape", "a/b", "a\\b", ".", "a b"] {
            assert!(matches!(
                CommandReceiptStore::new(temp.path()).execute::<Value, _>(
                    id,
                    "approve",
                    &json!({}),
                    || panic!("invalid id")
                ),
                Err(CommandReceiptError::InvalidId)
            ));
        }
        assert!(!temp.path().join(".zest").exists());
    }

    #[test]
    fn null_outcome_is_completed_and_storage_failure_prevents_execution() {
        let temp = tempfile::tempdir().unwrap();
        let store = CommandReceiptStore::new(temp.path());
        store
            .execute("cmd-null", "action", &json!({}), || Value::Null)
            .unwrap();
        assert!(store
            .execute::<Value, _>("cmd-null", "action", &json!({}), || panic!(
                "null was completed"
            ))
            .unwrap()
            .is_null());
        let broken = tempfile::tempdir().unwrap();
        std::fs::write(broken.path().join(".zest"), "not a directory").unwrap();
        assert!(matches!(
            CommandReceiptStore::new(broken.path()).execute::<Value, _>(
                "cmd-1",
                "action",
                &json!({}),
                || panic!("storage failed")
            ),
            Err(CommandReceiptError::Storage(_))
        ));
    }

    #[test]
    fn command_ids_are_case_sensitive_and_allow_windows_device_names() {
        let temp = tempfile::tempdir().unwrap();
        let store = CommandReceiptStore::new(temp.path());
        for id in ["cmd", "CMD", "CON", "NUL"] {
            let result = store
                .execute(id, "approve", &json!({}), || json!(id))
                .unwrap();
            assert_eq!(result, id);
        }
        assert_eq!(
            std::fs::read_dir(&store.dir)
                .unwrap()
                .filter(|entry| entry
                    .as_ref()
                    .unwrap()
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "json"))
                .count(),
            4
        );
        assert_eq!(
            store
                .execute::<Value, _>("cmd", "approve", &json!({}), || panic!("retry"))
                .unwrap(),
            "cmd"
        );
    }

    #[test]
    fn object_key_order_does_not_change_command_identity() {
        let temp = tempfile::tempdir().unwrap();
        let store = CommandReceiptStore::new(temp.path());
        let original: Value = serde_json::from_str(
            r#"{"worker":{"kind":"provider","providerId":"fixture"},"jobId":"job-1"}"#,
        )
        .unwrap();
        let reordered: Value = serde_json::from_str(
            r#"{"jobId":"job-1","worker":{"providerId":"fixture","kind":"provider"}}"#,
        )
        .unwrap();
        store
            .execute("cmd-1", "approve", &original, || json!("first result"))
            .unwrap();
        assert_eq!(
            store
                .execute::<Value, _>("cmd-1", "approve", &reordered, || panic!("same arguments"))
                .unwrap(),
            "first result"
        );
        assert!(matches!(
            store.execute::<Value, _>(
                "cmd-1",
                "approve",
                &json!({"jobId":"different"}),
                || panic!("changed arguments")
            ),
            Err(CommandReceiptError::Conflict)
        ));
    }

    #[test]
    fn outcome_persistence_failure_leaves_intent_and_never_reexecutes() {
        struct Unsavable;
        impl Serialize for Unsavable {
            fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("fixture serialization failure"))
            }
        }
        impl<'de> Deserialize<'de> for Unsavable {
            fn deserialize<D: serde::Deserializer<'de>>(_: D) -> Result<Self, D::Error> {
                Err(serde::de::Error::custom("fixture cannot load outcome"))
            }
        }
        let temp = tempfile::tempdir().unwrap();
        let store = CommandReceiptStore::new(temp.path());
        let mut executed = false;
        assert!(matches!(
            store.execute("cmd-1", "approve", &json!({}), || {
                executed = true;
                Unsavable
            }),
            Err(CommandReceiptError::Storage(_))
        ));
        assert!(executed);
        assert!(matches!(
            store.execute::<Value, _>("cmd-1", "approve", &json!({}), || panic!(
                "outcome was lost"
            )),
            Err(CommandReceiptError::Incomplete)
        ));
    }

    #[test]
    fn receipts_stay_untracked_in_a_project_without_ignore_rules() {
        let temp = tempfile::tempdir().unwrap();
        let init = std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(temp.path())
            .output()
            .unwrap();
        assert!(
            init.status.success(),
            "{}",
            String::from_utf8_lossy(&init.stderr)
        );
        CommandReceiptStore::new(temp.path())
            .execute(
                "cmd-1",
                "approve",
                &json!({}),
                || json!({"status":"queued"}),
            )
            .unwrap();
        let untracked = std::process::Command::new("git")
            .args(["ls-files", "--others", "--exclude-standard"])
            .current_dir(temp.path())
            .output()
            .unwrap();
        assert!(untracked.status.success());
        assert!(
            untracked.stdout.is_empty(),
            "{}",
            String::from_utf8_lossy(&untracked.stdout)
        );
    }
}
