//! Keeping oversized tool results out of the model's context without destroying
//! them.
//!
//! Every tool here bounds its own output with a private constant, and until now
//! the bytes past that constant were simply gone: the model was told some were
//! omitted and had no way to ever see them. This module writes the full result to
//! a session-scoped file and hands the model a bounded preview plus a locator it
//! can `read_file` or `grep`, so a truncation becomes a retrieval instead of a
//! loss.
//!
//! # What "full result" means
//!
//! The complete string the tool *returned*. A dispatch-level policy sits after
//! the tool, so it cannot recover what a tool discarded earlier: `bash` drops the
//! middle of a stream while the command is still running, and `grep` stops
//! walking once its budget is spent. Those losses are announced inside the string
//! this module stores. Reading the notice as a promise to reconstruct them would
//! be reading it wrong.
//!
//! # Disposability
//!
//! Artifacts are written without an fsync and swept on age and size, so a
//! locator can outlive its bytes — after a crash, or after a later turn's sweep.
//! That is the intended trade: the preview is in the transcript either way, and a
//! stale locator costs the model one failed read it can adapt to. Storage is not
//! durable state and must never be treated as such.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::approval::ToolRisk;
use super::outcome::ToolOutcome;
use super::read_file::{MAX_BYTES as READ_REACH_BYTES, READ_FILE_TOOL};
use crate::thread::{new_id, ThreadId};

/// Directory under `.zest` that holds every conversation's artifacts.
const SPILL_DIR: &str = "spill";

/// Ceiling on one conversation's artifacts, matching the checkpoint store's.
const MAX_DIR_BYTES: u64 = 64 * 1024 * 1024;
/// Ceiling on one conversation's artifact count.
const MAX_FILES: usize = 64;
/// Age past which an artifact is assumed abandoned.
const MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Storage for oversized tool results, scoped to one conversation.
///
/// Construction performs no I/O and creates no directory: a runtime is built for
/// `doctor`, for smoke tests, and for every CLI invocation, and none of those
/// should leave a directory behind for output that never spilled.
pub struct SpillStore {
    dir: PathBuf,
    /// Project-relative, forward slashes — the form the read tools accept.
    locator_prefix: String,
}

impl SpillStore {
    /// Validate the conversation id and compute paths.
    ///
    /// Reuses [`ThreadId`] rather than adding a third charset validator to the
    /// crate; the newtype is what makes a path unbuildable from an unvalidated
    /// string.
    pub fn open(workspace_root: impl AsRef<Path>, thread_id: &str) -> Result<Self, String> {
        let id = ThreadId::parse(thread_id)?;
        let dir = workspace_root
            .as_ref()
            .join(".zest")
            .join(SPILL_DIR)
            .join(id.as_str());
        let store = Self {
            dir,
            locator_prefix: format!(".zest/{SPILL_DIR}/{id}"),
        };
        store.sweep_siblings();
        Ok(store)
    }

    /// Where a model should look, as a project-relative locator.
    fn locator_prefix(&self) -> &str {
        &self.locator_prefix
    }

    /// A unique file name for one result. No I/O, so a caller can compose the
    /// locator into a notice and *then* decide whether the notice fits.
    pub fn next_name(&self, tool_name: &str) -> String {
        format!("{}-{}.txt", new_id("spill"), sanitize_tool(tool_name))
    }

    /// The locator a given name will have.
    pub fn locator(&self, name: &str) -> String {
        format!("{}/{name}", self.locator_prefix)
    }

    /// Persist one result. `None` means nothing was stored and the caller must
    /// keep its original body.
    ///
    /// Best-effort by construction: a failure here must never turn a successful
    /// tool call into a failed one, so every error path warns and returns `None`.
    ///
    /// Written inline with no await between naming and writing, which is what
    /// makes a cancelled call unable to leave a half-written artifact — a
    /// detached write would complete after the future was dropped and orphan a
    /// file for a result the model never saw. No fsync and no temp-and-rename:
    /// the name is unique so nothing is replaced and there is no torn reader,
    /// and a disk sync per tool call would be a new stall in the turn loop paid
    /// for a file this module already documents as disposable.
    pub fn write(&self, name: &str, body: &str) -> Option<()> {
        // `name` is expected to come from `next_name`, which sanitizes it — but
        // this is public and takes a bare string, so it checks rather than
        // trusts. One ordinary path component, nothing else: that rejects `..`,
        // an absolute path, a drive prefix, and anything with a separator.
        //
        // Both separators are spelled out because `Path::components` only splits
        // on the ones the *host* recognises. On Unix a backslash is an ordinary
        // filename byte, so `a\b.txt` arrives as a single Normal component and
        // would be accepted there while being refused on Windows. The guard is
        // about what the name means as a path rather than what this build's
        // separator happens to be, so it has to be the same rule on both.
        let mut parts = Path::new(name).components();
        let single_segment = !name.contains(['/', '\\'])
            && matches!(parts.next(), Some(std::path::Component::Normal(_)))
            && parts.next().is_none();
        if !single_segment {
            eprintln!("warning: refusing to store tool output under `{name}`");
            return None;
        }
        if let Err(error) = fs::create_dir_all(&self.dir) {
            eprintln!("warning: could not create {}: {error}", self.dir.display());
            return None;
        }
        let path = self.dir.join(name);
        match fs::File::create(&path).and_then(|mut file| file.write_all(body.as_bytes())) {
            Ok(()) => {
                self.sweep(name);
                Some(())
            }
            Err(error) => {
                eprintln!(
                    "warning: could not store tool output in {}: {error}",
                    path.display()
                );
                let _ = fs::remove_file(&path);
                None
            }
        }
    }

    /// Drop abandoned and over-budget artifacts, oldest first.
    ///
    /// `keep` is the file just written — the one a notice already points at, and
    /// therefore the one artifact that must survive its own sweep. Unlike the
    /// checkpoint pruner there is no "always leave one": an artifact nobody asked
    /// for is not a restore point.
    fn sweep(&self, keep: &str) {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        let now = SystemTime::now();
        let mut files: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
        // The kept file is never a deletion candidate, but it still occupies the
        // directory — leaving it out of the total would enforce the byte ceiling
        // against everything *except* the newest artifact, so a single large
        // spill could sit over budget until some later write happened to count it.
        let mut kept_bytes = 0u64;
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            if entry.file_name() == std::ffi::OsStr::new(keep) {
                kept_bytes = meta.len();
                continue;
            }
            let modified = meta.modified().unwrap_or(now);
            if now.duration_since(modified).is_ok_and(|age| age > MAX_AGE) {
                let _ = fs::remove_file(entry.path());
                continue;
            }
            files.push((modified, meta.len(), entry.path()));
        }

        files.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.2.cmp(&b.2)));
        let mut total: u64 = kept_bytes + files.iter().map(|(_, len, _)| *len).sum::<u64>();
        let mut count = files.len();
        for (_, len, path) in &files {
            if count < MAX_FILES && total <= MAX_DIR_BYTES {
                break;
            }
            if fs::remove_file(path).is_ok() {
                total = total.saturating_sub(*len);
                count -= 1;
            }
        }
    }

    /// Remove other conversations' abandoned directories.
    ///
    /// The only reclaim path for a CLI session, which has no durable thread and
    /// so never reaches [`remove_thread_dir`], and the backstop for a directory
    /// whose deletion failed. Creates nothing: a missing spill root is simply an
    /// unreadable directory here.
    fn sweep_siblings(&self) {
        let Some(root) = self.dir.parent() else {
            return;
        };
        let Ok(entries) = fs::read_dir(root) else {
            return;
        };
        let now = SystemTime::now();
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if path == self.dir || !meta.is_dir() {
                continue;
            }
            // An empty directory falls back to its *own* age rather than counting
            // as residue outright. Another live session creates its directory
            // before writing into it, and treating that momentarily-empty state
            // as abandoned would delete a conversation's store out from under it.
            let newest = fs::read_dir(&path)
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|child| child.metadata().ok()?.modified().ok())
                .max()
                .or_else(|| meta.modified().ok());
            let stale = newest
                .is_some_and(|newest| now.duration_since(newest).is_ok_and(|age| age > MAX_AGE));
            if stale {
                let _ = fs::remove_dir_all(&path);
            }
        }
    }
}

/// Keep an artifact name boring whatever the tool is called.
///
/// The registry found the tool by this name, so it is already one of ours —
/// this is insurance against a future registration, not validation.
fn sanitize_tool(tool_name: &str) -> String {
    let cleaned: String = tool_name
        .chars()
        .take(40)
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "tool".to_string()
    } else {
        cleaned
    }
}

/// Remove one conversation's artifacts. Missing is success.
///
/// `zest_dir` is the `.zest` directory itself, so this composes with the thread
/// store's own cleanup.
pub fn remove_thread_dir(zest_dir: &Path, thread_id: &ThreadId) {
    let dir = zest_dir.join(SPILL_DIR).join(thread_id.as_str());
    if let Err(error) = fs::remove_dir_all(&dir) {
        if error.kind() != std::io::ErrorKind::NotFound {
            eprintln!("warning: could not remove {}: {error}", dir.display());
        }
    }
}

/// Decides *when* a result is too large to stay inline, and composes the notice.
///
/// Owns no storage and no preview mechanics: bounding is [`crate::bounded`] and
/// storage is [`SpillStore`].
pub struct SpillPolicy {
    store: SpillStore,
    cap: usize,
}

impl SpillPolicy {
    pub fn new(store: SpillStore, cap: usize) -> Self {
        Self { store, cap }
    }

    pub fn spill_dir(&self) -> &str {
        self.store.locator_prefix()
    }

    /// Replace an oversized result with a preview, a locator, and a retrieval
    /// hint — or return it untouched.
    ///
    /// Order is name, clip, write, replace. Clipping before writing is what makes
    /// "spill nothing" mean it: deciding after the write would leave an
    /// unreferenced artifact behind whenever the notice could not fit.
    pub fn apply(&self, tool: &str, risk: ToolRisk, outcome: ToolOutcome) -> ToolOutcome {
        if !self.should_spill(tool, risk, &outcome.body) {
            return outcome;
        }

        let total = outcome.body.len();
        let name = self.store.next_name(tool);
        let locator = self.store.locator(&name);

        let Some(replacement) = crate::bounded::ends_within(&outcome.body, self.cap, |omitted| {
            notice(omitted, &locator, total)
        }) else {
            return outcome;
        };
        if self.store.write(&name, &outcome.body).is_none() {
            return outcome;
        }

        // Only the body moves. `metadata` carries delegation provenance the
        // ledger and the UI both read, and it never went on the wire anyway.
        ToolOutcome {
            body: replacement,
            ..outcome
        }
    }

    fn should_spill(&self, tool: &str, risk: ToolRisk, body: &str) -> bool {
        if self.cap == 0 || body.len() <= self.cap {
            return false;
        }
        // The retrieval hint names `read_file`, so spilling its output would be a
        // read that spills into a read that spills.
        if tool == READ_FILE_TOOL {
            return false;
        }
        // A sensitive result already has its UI summary suppressed and is
        // redacted out of the delegation handoff. A spill would write a second
        // cleartext copy that neither mechanism knows about, and print its path
        // into the model-facing body.
        if risk == ToolRisk::Sensitive {
            return false;
        }
        true
    }
}

/// What the model is told in place of the bytes.
///
/// Names this repo's tools and parameters rather than generic ones, and admits
/// the reach limit: `read_file` counts its budget from byte zero and applies any
/// offset afterwards, and `grep` bounds each file the same way *silently*. A
/// model that believes it can retrieve the whole artifact will act on the part it
/// could not reach.
fn notice(omitted: usize, locator: &str, total: usize) -> String {
    let mut out = format!(
        "\n\n(Omitted {omitted} bytes. Full result stored at: {locator}. \
         Use read_file with offset/limit, or grep with `path` set to that file, \
         to search within it.)\n\n"
    );
    if total > READ_REACH_BYTES {
        out.push_str(&format!(
            "(That file is {total} bytes; read_file and grep only reach its first \
             {READ_REACH_BYTES}.)\n\n"
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        // Unique per call: the concurrency test runs several stores in one
        // process, so a fixed name that gets removed up front is not safe here.
        let dir = std::env::temp_dir().join(format!("zest-spill-{name}-{}", new_id("t")));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn store(root: &Path) -> SpillStore {
        SpillStore::open(root, "t-1").unwrap()
    }

    #[test]
    fn a_locator_is_project_relative_with_forward_slashes() {
        let root = scratch("locator");
        let store = store(&root);
        assert_eq!(store.locator_prefix(), ".zest/spill/t-1");
        let locator = store.locator("spill-abc-1-grep.txt");
        assert_eq!(locator, ".zest/spill/t-1/spill-abc-1-grep.txt");
        assert!(!locator.contains('\\'), "{locator}");
    }

    #[test]
    fn opening_a_store_creates_no_directory() {
        let root = scratch("lazy");
        let _store = store(&root);
        assert!(
            !root.join(".zest").exists(),
            "a store that never spilled must leave no directory behind"
        );
    }

    #[test]
    fn an_invalid_thread_id_is_refused() {
        let root = scratch("badid");
        for bad in ["../escape", "a/b", "C:evil", "", "."] {
            assert!(
                SpillStore::open(&root, bad).is_err(),
                "accepted {bad:?} as a thread id"
            );
        }
    }

    #[test]
    fn a_stored_file_holds_the_whole_body() {
        let root = scratch("whole");
        let store = store(&root);
        let body = "line\n".repeat(50_000);
        let name = store.next_name("grep");
        assert!(store.write(&name, &body).is_some());
        let back = fs::read_to_string(root.join(".zest/spill/t-1").join(&name)).unwrap();
        assert_eq!(back, body);
    }

    #[test]
    fn a_file_name_encodes_the_tool() {
        let root = scratch("named");
        let store = store(&root);
        let name = store.next_name("web_search");
        assert!(name.ends_with("-web_search.txt"), "{name}");
        assert!(name.starts_with("spill-"), "{name}");
    }

    #[test]
    fn a_tool_name_with_separators_cannot_escape_the_spill_directory() {
        let root = scratch("escape");
        let store = store(&root);
        let name = store.next_name("../../etc/passwd");
        assert!(!name.contains('/') && !name.contains('\\'), "{name}");
        assert!(store.write(&name, "x").is_some());
        assert!(root.join(".zest/spill/t-1").join(&name).is_file());
    }

    #[test]
    fn an_empty_tool_name_still_produces_a_file() {
        let root = scratch("emptyname");
        let store = store(&root);
        assert!(store.next_name("").ends_with("-tool.txt"));
        assert!(store.next_name("///").ends_with("-___.txt"));
    }

    #[tokio::test]
    async fn concurrent_stores_in_one_turn_do_not_collide() {
        let root = scratch("concurrent");
        let store = std::sync::Arc::new(store(&root));
        let writes = (0..32).map(|i| {
            let store = store.clone();
            async move {
                let name = store.next_name("bash");
                store.write(&name, &format!("body {i}"));
                name
            }
        });
        let names: Vec<String> = futures_util::future::join_all(writes).await;
        let unique: std::collections::HashSet<&String> = names.iter().collect();
        assert_eq!(unique.len(), 32, "names collided: {names:?}");
        let on_disk = fs::read_dir(root.join(".zest/spill/t-1")).unwrap().count();
        assert_eq!(on_disk, 32);
    }

    #[test]
    fn a_name_that_is_not_a_single_path_segment_is_refused() {
        let root = scratch("badname");
        let store = store(&root);
        for bad in ["../escape.txt", "a/b.txt", "a\\b.txt", "..", "/abs.txt", ""] {
            assert!(
                store.write(bad, "body").is_none(),
                "accepted {bad:?} as a file name"
            );
        }
        assert!(!root.join("escape.txt").exists());
        assert!(!root.join(".zest/spill/escape.txt").exists());
    }

    #[test]
    fn the_byte_ceiling_counts_the_file_just_written() {
        let root = scratch("keepcounted");
        let store = store(&root);
        // One artifact larger than the whole directory budget. It survives its
        // own sweep, but a later write must see the directory as over budget
        // rather than measuring everything except the newest file.
        let huge = "x".repeat((MAX_DIR_BYTES + 1) as usize);
        let first = store.next_name("bash");
        assert!(store.write(&first, &huge).is_some());

        let second = store.next_name("bash");
        assert!(store.write(&second, "small").is_some());

        let dir = root.join(".zest/spill/t-1");
        assert!(dir.join(&second).exists(), "the newest artifact was swept");
        assert!(
            !dir.join(&first).exists(),
            "the over-budget artifact survived a sweep that could see it"
        );
    }

    #[test]
    fn a_write_failure_returns_none() {
        let root = scratch("blocked");
        // A file where the directory needs to be: `create_dir_all` fails on both
        // platforms, which is the "no spill directory" path.
        fs::create_dir_all(root.join(".zest/spill")).unwrap();
        fs::write(root.join(".zest/spill/t-1"), "in the way").unwrap();
        let store = store(&root);
        assert!(store.write(&store.next_name("bash"), "body").is_none());
    }

    #[test]
    fn an_over_budget_directory_is_pruned_oldest_first() {
        let root = scratch("prune");
        let store = store(&root);
        let dir = root.join(".zest/spill/t-1");
        let mut names = Vec::new();
        let total = MAX_FILES + 8;
        for i in 0..total {
            let name = store.next_name("bash");
            assert!(store.write(&name, &format!("body {i}")).is_some());
            // Linux tmpfs and overlay often give every file the same mtime when
            // writes land in one burst. The next sweep would then follow
            // readdir order, which is not insertion order, and names[0] survived.
            let path = dir.join(&name);
            let ts = SystemTime::now() - Duration::from_secs((total - i) as u64);
            let file = fs::OpenOptions::new().write(true).open(&path).unwrap();
            file.set_times(fs::FileTimes::new().set_modified(ts))
                .unwrap();
            names.push(name);
        }
        let left = fs::read_dir(&dir).unwrap().count();
        // The budget bounds the directory including the file just written.
        assert!(left <= MAX_FILES, "{left} files survived the sweep");
        assert!(left < names.len(), "nothing was swept at all");
        // The oldest went first; the newest is still there.
        assert!(!dir.join(&names[0]).exists(), "oldest was kept");
        assert!(dir.join(names.last().unwrap()).exists(), "newest was swept");
    }

    #[test]
    fn pruning_never_removes_the_file_just_written() {
        let root = scratch("keepnew");
        let store = store(&root);
        for _ in 0..MAX_FILES * 2 {
            let name = store.next_name("bash");
            assert!(store.write(&name, "body").is_some());
            assert!(
                root.join(".zest/spill/t-1").join(&name).is_file(),
                "the notice points at {name}, so it must survive its own sweep"
            );
        }
    }

    #[test]
    /// The window between another session's `create_dir_all` and its first write
    /// is real, and deleting a live conversation's store during it would be very
    /// hard to trace back to here.
    fn a_freshly_created_sibling_directory_is_not_mistaken_for_residue() {
        let root = scratch("siblings");
        let live = root.join(".zest/spill/t-brand-new");
        fs::create_dir_all(&live).unwrap();
        let _store = store(&root);
        assert!(
            live.exists(),
            "another session's directory was swept moments after it was created"
        );
    }

    /// Backdate a file past the age ceiling.
    fn make_ancient(path: &Path) {
        let long_ago = SystemTime::now() - MAX_AGE - Duration::from_secs(60);
        let file = fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_times(fs::FileTimes::new().set_modified(long_ago))
            .unwrap();
    }

    #[test]
    fn a_long_abandoned_sibling_directory_is_swept() {
        let root = scratch("siblings-stale");
        let stale = root.join(".zest/spill/t-old");
        fs::create_dir_all(&stale).unwrap();
        let artifact = stale.join("spill-x-1-bash.txt");
        fs::write(&artifact, "old").unwrap();
        make_ancient(&artifact);

        let _store = store(&root);
        assert!(
            !stale.exists(),
            "an abandoned conversation's artifacts were kept forever"
        );
    }

    #[test]
    fn an_abandoned_artifact_is_swept_from_this_conversation_too() {
        let root = scratch("age");
        let store = store(&root);
        let old = store.next_name("bash");
        assert!(store.write(&old, "old").is_some());
        make_ancient(&root.join(".zest/spill/t-1").join(&old));

        // Any later write triggers the sweep.
        let fresh = store.next_name("bash");
        assert!(store.write(&fresh, "fresh").is_some());

        let dir = root.join(".zest/spill/t-1");
        assert!(!dir.join(&old).exists(), "the stale artifact survived");
        assert!(dir.join(&fresh).exists(), "the new artifact was swept");
    }

    #[test]
    fn a_live_sibling_directory_survives() {
        let root = scratch("livesibling");
        let other = root.join(".zest/spill/t-other");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("spill-x-1-bash.txt"), "recent").unwrap();
        let _store = store(&root);
        assert!(other.exists(), "a recently used conversation was swept");
    }

    #[test]
    fn removing_a_thread_directory_is_idempotent() {
        let root = scratch("remove");
        let store = store(&root);
        assert!(store.write(&store.next_name("bash"), "body").is_some());
        let id = ThreadId::parse("t-1").unwrap();
        let zest = root.join(".zest");
        remove_thread_dir(&zest, &id);
        assert!(!zest.join("spill/t-1").exists());
        // Second call must not warn or fail.
        remove_thread_dir(&zest, &id);
    }

    #[test]
    fn the_notice_admits_the_reach_limit_only_when_it_applies() {
        let small = notice(10, ".zest/spill/t-1/x.txt", 100_000);
        assert!(small.contains("Full result stored at"), "{small}");
        assert!(!small.contains("only reach"), "{small}");

        let big = notice(10, ".zest/spill/t-1/x.txt", 900_000);
        assert!(big.contains("only reach"), "{big}");
        assert!(big.contains("900000"), "{big}");
    }

    fn policy(root: &Path, cap: usize) -> SpillPolicy {
        SpillPolicy::new(store(root), cap)
    }

    #[test]
    fn a_body_within_the_cap_is_untouched() {
        let root = scratch("small");
        let policy = policy(&root, 1_024);
        let out = policy.apply("grep", ToolRisk::Read, ToolOutcome::text("short"));
        assert_eq!(out.body, "short");
        assert!(!root.join(".zest").exists(), "nothing should have spilled");
    }

    #[test]
    fn an_oversized_body_is_replaced_with_a_preview_and_a_locator() {
        let root = scratch("replaced");
        let policy = policy(&root, 2_048);
        let body = format!("HEAD{}TAIL", "x".repeat(50_000));
        let out = policy.apply("grep", ToolRisk::Read, ToolOutcome::text(body.clone()));
        // The notice sits at the seam, so both ends of the original are intact
        // and the join between them is named rather than left to be inferred.
        assert!(out.body.starts_with("HEAD"), "{}", out.body);
        assert!(out.body.ends_with("TAIL"), "{}", out.body);
        assert!(out.body.contains("Full result stored at"), "{}", out.body);

        // The locator the model was handed resolves to the whole body.
        let path = out
            .body
            .split("Full result stored at: ")
            .nth(1)
            .and_then(|rest| rest.split(". Use").next())
            .expect("the notice must carry a locator");
        assert!(path.starts_with(".zest/spill/t-1/"), "{path}");
        assert_eq!(fs::read_to_string(root.join(path)).unwrap(), body);
    }

    #[test]
    fn a_replacement_never_exceeds_the_cap() {
        let root = scratch("cap");
        for cap in [512, 2_048, 8_192, 32 * 1024] {
            let policy = policy(&root, cap);
            let out = policy.apply(
                "grep",
                ToolRisk::Read,
                ToolOutcome::text("x".repeat(500_000)),
            );
            assert!(
                out.body.len() <= cap,
                "cap {cap} produced {}",
                out.body.len()
            );
        }
    }

    #[test]
    fn a_replacement_is_always_smaller_than_the_original() {
        let root = scratch("smaller");
        let policy = policy(&root, 4_096);
        let body = "x".repeat(500_000);
        let out = policy.apply("grep", ToolRisk::Read, ToolOutcome::text(body.clone()));
        assert!(out.body.len() < body.len());
    }

    #[test]
    fn the_read_tool_is_never_spilled() {
        let root = scratch("readexempt");
        let policy = policy(&root, 1_024);
        let body = "x".repeat(50_000);
        let out = policy.apply(
            READ_FILE_TOOL,
            ToolRisk::Read,
            ToolOutcome::text(body.clone()),
        );
        assert_eq!(out.body, body, "a read that spills would spill into a read");
        assert!(!root.join(".zest").exists());
    }

    #[test]
    fn a_sensitive_result_is_never_spilled() {
        let root = scratch("sensitive");
        let policy = policy(&root, 1_024);
        let body = "PRIVATE KEY".repeat(5_000);
        let out = policy.apply(
            "read_file_secret",
            ToolRisk::Sensitive,
            ToolOutcome::text(body.clone()),
        );
        assert_eq!(out.body, body);
        assert!(
            !root.join(".zest").exists(),
            "a second cleartext copy must never be written"
        );
    }

    #[test]
    fn a_cap_too_small_for_the_notice_keeps_the_original_body() {
        let root = scratch("tinycap");
        let policy = policy(&root, 8);
        let body = "x".repeat(50_000);
        let out = policy.apply("grep", ToolRisk::Read, ToolOutcome::text(body.clone()));
        assert_eq!(out.body, body);
        assert!(
            !root.join(".zest").exists(),
            "nothing may be written when no replacement can be emitted"
        );
    }

    #[test]
    fn a_zero_cap_disables_the_policy() {
        let root = scratch("zerocap");
        let policy = policy(&root, 0);
        let body = "x".repeat(50_000);
        let out = policy.apply("grep", ToolRisk::Read, ToolOutcome::text(body.clone()));
        assert_eq!(out.body, body);
        assert!(!root.join(".zest").exists());
    }

    #[test]
    fn a_spill_write_failure_returns_the_original_body() {
        let root = scratch("writefail");
        fs::create_dir_all(root.join(".zest/spill")).unwrap();
        fs::write(root.join(".zest/spill/t-1"), "in the way").unwrap();
        let policy = policy(&root, 2_048);
        let body = "x".repeat(50_000);
        let out = policy.apply("grep", ToolRisk::Read, ToolOutcome::text(body.clone()));
        assert_eq!(out.body, body, "a storage failure must not hide the result");
    }

    #[test]
    fn metadata_survives_a_spill() {
        let root = scratch("metadata");
        let policy = policy(&root, 2_048);
        let metadata = super::super::outcome::ToolMetadata::Delegation {
            provider_id: "claude_code".into(),
            model: "sonnet".into(),
            diff: None,
            usage: None,
            job_id: None,
            stage: None,
            attempt: None,
            review_status: None,
        };
        let outcome = ToolOutcome::with_metadata("x".repeat(50_000), metadata.clone());
        let out = policy.apply("delegate_external", ToolRisk::Exec, outcome);
        assert!(out.body.contains("Full result stored at"));
        assert_eq!(out.metadata, Some(metadata));
    }
}
