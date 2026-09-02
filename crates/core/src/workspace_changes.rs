//! Read-only Git workspace change inspection.
//!
//! This module owns the expensive and security-sensitive part of branch review:
//! comparing a thread's starting commit with the current checkout, adding
//! untracked files, redacting likely-secret paths, and producing a bounded
//! patch for the desktop. Callers do not need to know which Git commands or
//! path cases are involved.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::time::timeout;

use crate::error::{HarnessError, Result};
use crate::tools::sensitive::is_sensitive_path;

/// Maximum patch payload sent to a desktop window in one change snapshot.
/// Large repositories still expose complete file/count metadata and mark the
/// visible patch as truncated.
pub const MAX_DISPLAY_DIFF_BYTES: usize = 2 * 1024 * 1024;
const GIT_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const INSPECTION_CACHE_CAPACITY: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
struct InspectionCacheKey {
    root: String,
    base_commit: Option<String>,
    base_branch: Option<String>,
    fingerprint: String,
}

#[derive(Debug, Clone)]
struct CachedInspection {
    key: InspectionCacheKey,
    snapshot: WorkspaceChangeSet,
}

fn inspection_cache() -> &'static Mutex<VecDeque<CachedInspection>> {
    static CACHE: OnceLock<Mutex<VecDeque<CachedInspection>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(VecDeque::with_capacity(INSPECTION_CACHE_CAPACITY)))
}

/// Git revisions are captured from `rev-parse HEAD` and persisted with a
/// thread. Keep accepting abbreviated hashes for older threads, but never pass
/// an option-like value into Git's revision parser.
pub fn is_safe_commit_id(value: &str) -> bool {
    let value = value.trim();
    (4..=64).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Hosts pass a numeric pull-request id into `gh`. Reject zero and anything
/// large enough that it is no longer a plausible issue number.
pub fn is_safe_pr_number(number: u64) -> bool {
    (1..=1_000_000).contains(&number)
}

/// Branch and remote-ish refs used as `git diff <ref>...HEAD`. Option-like
/// values and path traversal are rejected so the range cannot become a flag.
pub fn is_safe_git_ref(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() || value.len() > 255 || value.starts_with('-') || value.starts_with('.') {
        return false;
    }
    if value.contains("..") || value.contains('\\') || value.contains('\0') {
        return false;
    }
    value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'_' | b'-' | b'.'))
}

/// Turn a unified diff (from `gh pr diff` or `git diff A...HEAD`) into the
/// same bounded, redacted snapshot the workspace inspector returns.
pub fn snapshot_from_unified_diff(
    repository: impl Into<String>,
    raw: &[u8],
    base_branch: Option<&str>,
    branch: Option<&str>,
) -> WorkspaceChangeSet {
    let redacted = redact_diff(raw);
    let (diff, truncated) = bounded_utf8(&redacted, MAX_DISPLAY_DIFF_BYTES);
    let mut summaries = Vec::new();
    fill_stats_from_diff(&mut summaries, raw);
    for summary in &mut summaries {
        if is_sensitive_path(&summary.path) {
            summary.sensitive = true;
        }
    }
    WorkspaceChangeSet {
        change_id: blake3::hash(raw).to_hex().to_string(),
        repository: repository.into(),
        base_commit: None,
        base_branch: base_branch
            .filter(|value| is_safe_git_ref(value))
            .map(str::to_string),
        branch: branch
            .filter(|value| is_safe_git_ref(value))
            .map(str::to_string),
        additions: summaries.iter().map(|file| file.additions).sum(),
        deletions: summaries.iter().map(|file| file.deletions).sum(),
        changed_files: summaries,
        diff,
        truncated,
        unavailable: false,
    }
}

/// Compare `base_ref...HEAD` the way a pull request compares its merge base.
pub async fn inspect_merge_base(
    root: impl AsRef<Path>,
    base_ref: &str,
) -> Result<WorkspaceChangeSet> {
    let root = root.as_ref();
    if !is_safe_git_ref(base_ref) {
        return Ok(WorkspaceChangeSet::unavailable("invalid_base_ref"));
    }
    let range = format!("{base_ref}...HEAD");
    let output = match run_git(
        root,
        &[
            "diff",
            "--no-ext-diff",
            "--find-renames",
            "--binary",
            range.as_str(),
            "--",
        ],
    )
    .await
    {
        Ok(output) if output.status.success() || output.status.code() == Some(1) => output,
        _ => return Ok(WorkspaceChangeSet::unavailable("git")),
    };
    let branch = git_branch(root).await;
    Ok(snapshot_from_unified_diff(
        "git",
        &output.stdout,
        Some(base_ref),
        branch.as_deref(),
    ))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileChangeSummary {
    pub path: String,
    pub status: String,
    pub additions: u64,
    pub deletions: u64,
    pub binary: bool,
    pub sensitive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceChangeSet {
    /// Stable identity for the effective checkout state used to suppress
    /// repeated auto-opening of the same review.
    pub change_id: String,
    pub repository: String,
    pub base_commit: Option<String>,
    pub base_branch: Option<String>,
    pub branch: Option<String>,
    pub changed_files: Vec<FileChangeSummary>,
    pub additions: u64,
    pub deletions: u64,
    pub diff: String,
    pub truncated: bool,
    pub unavailable: bool,
}

impl WorkspaceChangeSet {
    pub fn empty(repository: impl Into<String>) -> Self {
        Self {
            change_id: blake3::hash(b"clean").to_hex().to_string(),
            repository: repository.into(),
            base_commit: None,
            base_branch: None,
            branch: None,
            changed_files: Vec::new(),
            additions: 0,
            deletions: 0,
            diff: String::new(),
            truncated: false,
            unavailable: false,
        }
    }

    pub fn unavailable(repository: impl Into<String>) -> Self {
        let mut snapshot = Self::empty(repository);
        snapshot.change_id = blake3::hash(b"unavailable").to_hex().to_string();
        snapshot.unavailable = true;
        snapshot
    }

    pub fn has_changes(&self) -> bool {
        !self.changed_files.is_empty() || !self.diff.trim().is_empty()
    }
}

#[derive(Debug, Clone)]
struct StatusEntry {
    path: String,
    status: String,
    sensitive: bool,
}

/// Capture the cumulative branch/worktree patch for a thread.
pub async fn inspect(
    root: impl AsRef<Path>,
    base_commit: Option<&str>,
    base_branch: Option<&str>,
) -> Result<WorkspaceChangeSet> {
    let root = root.as_ref().to_path_buf();
    let base_commit = match base_commit.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) if is_safe_commit_id(value) => Some(value),
        Some(_) => return Ok(WorkspaceChangeSet::unavailable("invalid_base_commit")),
        None => None,
    };
    let probe = match run_git(&root, &["rev-parse", "--is-inside-work-tree"]).await {
        Ok(output) => output,
        Err(_) => return Ok(WorkspaceChangeSet::unavailable("git")),
    };
    if !probe.status.success() || String::from_utf8_lossy(&probe.stdout).trim() != "true" {
        return Ok(WorkspaceChangeSet::unavailable("not_git"));
    }

    let branch = git_branch(&root).await;
    let status = run_git(
        &root,
        &["status", "--porcelain=v1", "--untracked-files=all", "-z"],
    )
    .await?;
    if !status.status.success() {
        return Ok(WorkspaceChangeSet::unavailable("git"));
    }
    let entries = parse_status(&status.stdout);
    let head = git_head(&root).await;
    let fingerprint = workspace_fingerprint(&root, &head, &branch, &status.stdout, &entries).await;
    let cache_key = InspectionCacheKey {
        root: root.to_string_lossy().into_owned(),
        base_commit: base_commit.map(str::to_string),
        base_branch: base_branch.map(str::to_string),
        fingerprint,
    };
    if let Some(snapshot) = cached_inspection(&cache_key) {
        return Ok(snapshot);
    }

    let tracked_args = diff_args(base_commit);
    let mut raw_diff = match run_git(&root, &tracked_args).await {
        Ok(tracked) if tracked.status.success() || tracked.status.code() == Some(1) => {
            tracked.stdout
        }
        // A repository without its first commit has no `HEAD`. Staged changes
        // are still useful review material, so use the index as the fallback.
        Ok(_) | Err(_) if base_commit.is_none() => run_git(&root, &diff_cached_args())
            .await
            .map(|output| output.stdout)
            .unwrap_or_default(),
        Ok(_) | Err(_) => return Ok(WorkspaceChangeSet::unavailable("git")),
    };
    for entry in entries.iter().filter(|entry| entry.status == "??") {
        if is_sensitive_path(&entry.path) {
            raw_diff.extend_from_slice(
                format!(
                    "diff --git a/{0} b/{0}\n--- /dev/null\n+++ b/{0}\n@@ sensitive file omitted @@\n",
                    entry.path
                )
                .as_bytes(),
            );
            continue;
        }
        let output = run_git(
            &root,
            &[
                "diff",
                "--no-index",
                "--binary",
                "--",
                "/dev/null",
                &entry.path,
            ],
        )
        .await?;
        // `git diff --no-index` returns 1 when the files differ, which is the
        // normal result for an untracked file.
        if output.status.success() || output.status.code() == Some(1) {
            raw_diff.extend_from_slice(&output.stdout);
        }
    }

    let change_id = change_id(&root, base_commit, &status.stdout, &raw_diff).await;
    let redacted = redact_diff(&raw_diff);
    let (diff, truncated) = bounded_utf8(&redacted, MAX_DISPLAY_DIFF_BYTES);
    let mut summaries = summaries_from_status(&entries);
    fill_stats_from_diff(&mut summaries, &raw_diff);

    let snapshot = WorkspaceChangeSet {
        change_id,
        repository: "git".into(),
        base_commit: base_commit.map(str::to_string),
        base_branch: base_branch.map(str::to_string),
        branch,
        additions: summaries.iter().map(|file| file.additions).sum(),
        deletions: summaries.iter().map(|file| file.deletions).sum(),
        changed_files: summaries,
        diff,
        truncated,
        unavailable: false,
    };
    cache_inspection(cache_key, snapshot.clone());
    Ok(snapshot)
}

fn cached_inspection(key: &InspectionCacheKey) -> Option<WorkspaceChangeSet> {
    let mut cache = inspection_cache().lock().ok()?;
    let index = cache.iter().position(|entry| &entry.key == key)?;
    let entry = cache.remove(index)?;
    let snapshot = entry.snapshot.clone();
    cache.push_front(entry);
    Some(snapshot)
}

fn cache_inspection(key: InspectionCacheKey, snapshot: WorkspaceChangeSet) {
    let Ok(mut cache) = inspection_cache().lock() else {
        return;
    };
    cache.retain(|entry| entry.key != key);
    cache.push_front(CachedInspection { key, snapshot });
    cache.truncate(INSPECTION_CACHE_CAPACITY);
}

async fn git_head(root: &Path) -> Option<String> {
    let output = run_git(root, &["rev-parse", "HEAD"]).await.ok()?;
    if !output.status.success() {
        return None;
    }
    let head = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!head.is_empty()).then_some(head)
}

async fn workspace_fingerprint(
    root: &Path,
    head: &Option<String>,
    branch: &Option<String>,
    status: &[u8],
    entries: &[StatusEntry],
) -> String {
    let index_fingerprint = git_index_fingerprint(root, entries).await;
    let root_string = root.to_string_lossy().into_owned();
    let worktree_root = root.to_path_buf();
    let paths = entries
        .iter()
        .map(|entry| entry.path.clone())
        .collect::<Vec<_>>();
    let worktree_fingerprint =
        tokio::task::spawn_blocking(move || worktree_content_fingerprint(&worktree_root, &paths))
            .await
            .unwrap_or_else(|_| "worktree-fingerprint-unavailable".to_string());

    let mut hasher = blake3::Hasher::new();
    hasher.update(root_string.as_bytes());
    hasher.update(head.as_deref().unwrap_or_default().as_bytes());
    hasher.update(branch.as_deref().unwrap_or_default().as_bytes());
    hasher.update(status);
    hasher.update(index_fingerprint.as_bytes());
    hasher.update(worktree_fingerprint.as_bytes());

    hasher.finalize().to_hex().to_string()
}

async fn git_index_fingerprint(root: &Path, entries: &[StatusEntry]) -> String {
    if entries.is_empty() {
        return String::new();
    }

    let mut args = vec!["--literal-pathspecs", "ls-files", "--stage", "-z", "--"];
    args.extend(entries.iter().map(|entry| entry.path.as_str()));
    match run_git(root, &args).await {
        Ok(output) if output.status.success() => blake3::hash(&output.stdout).to_hex().to_string(),
        Ok(output) => format!("index-command-failed:{}", output.status),
        Err(_) => "index-command-unavailable".to_string(),
    }
}

fn worktree_content_fingerprint(root: &Path, paths: &[String]) -> String {
    let mut hasher = blake3::Hasher::new();

    // Git status is the cheap primary probe, but metadata alone can remain
    // unchanged when a file is rewritten with the same size and timestamp.
    // Hash the current worktree content for every reported path so a cache hit
    // can never return a stale diff for that class of edit.
    for path in paths {
        hasher.update(&(path.len() as u64).to_le_bytes());
        hasher.update(path.as_bytes());
        match std::fs::symlink_metadata(root.join(path)) {
            Ok(metadata) => {
                hasher.update(b"present");
                hasher.update(&metadata.len().to_le_bytes());
                hasher.update(&[metadata.is_file() as u8, metadata.is_dir() as u8]);
                hasher.update(&[metadata.permissions().readonly() as u8]);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    hasher.update(&metadata.permissions().mode().to_le_bytes());
                }

                if metadata.file_type().is_symlink() {
                    match std::fs::read_link(root.join(path)) {
                        Ok(target) => {
                            hasher.update(b"symlink-target");
                            hasher.update(target.to_string_lossy().as_bytes());
                        }
                        Err(_) => {
                            hasher.update(b"symlink-target-unavailable");
                        }
                    }
                } else if metadata.is_file() {
                    match hash_file(root.join(path)) {
                        Ok(content_hash) => {
                            hasher.update(b"content");
                            hasher.update(content_hash.as_bytes());
                        }
                        Err(_) => {
                            hasher.update(b"content-unavailable");
                        }
                    }
                }
            }
            Err(_) => {
                hasher.update(b"missing");
            }
        }
    }
    hasher.finalize().to_hex().to_string()
}

fn hash_file(path: impl AsRef<Path>) -> std::io::Result<String> {
    use std::io::Read;

    let mut file = std::fs::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn diff_args(base_commit: Option<&str>) -> Vec<&str> {
    let mut args = vec!["diff", "--no-ext-diff", "--find-renames", "--binary"];
    if let Some(base_commit) = base_commit.filter(|value| !value.trim().is_empty()) {
        args.push(base_commit);
    } else {
        // Comparing against HEAD includes both staged and unstaged tracked
        // changes. Untracked paths are appended separately below.
        args.push("HEAD");
    }
    args.extend(["--"]);
    args
}

fn diff_cached_args() -> Vec<&'static str> {
    vec![
        "diff",
        "--no-ext-diff",
        "--find-renames",
        "--binary",
        "--cached",
        "--",
    ]
}

async fn run_git(root: &Path, args: &[&str]) -> Result<std::process::Output> {
    let mut command = Command::new("git");
    command.args(args).current_dir(root).kill_on_drop(true);
    timeout(GIT_COMMAND_TIMEOUT, command.output())
        .await
        .map_err(|_| HarnessError::Other("git command timed out".into()))?
        .map_err(|error| HarnessError::Other(format!("could not run git: {error}")))
}

async fn git_branch(root: &Path) -> Option<String> {
    let output = run_git(root, &["symbolic-ref", "--short", "-q", "HEAD"])
        .await
        .ok()?;
    if output.status.success() {
        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !branch.is_empty() {
            return Some(branch);
        }
    }
    let output = run_git(root, &["rev-parse", "--short", "HEAD"])
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn parse_status(raw: &[u8]) -> Vec<StatusEntry> {
    let mut fields = raw.split(|byte| *byte == 0);
    let mut entries = Vec::new();
    while let Some(field) = fields.next() {
        if field.len() < 3 {
            continue;
        }
        let text = String::from_utf8_lossy(field);
        let code = &text[..2];
        let path = text[3..].to_string();
        // Porcelain -z emits `XY <destination>\0<source>\0` for renames and
        // copies. The destination is already in this field, so step over the
        // trailing source rather than reading it as another entry.
        if code.contains('R') || code.contains('C') {
            let _ = fields.next();
        }
        let status = if code == "??" {
            "untracked".to_string()
        } else if code.contains('D') {
            "deleted".to_string()
        } else if code.contains('R') {
            "renamed".to_string()
        } else if code.contains('A') {
            "added".to_string()
        } else {
            "modified".to_string()
        };
        entries.push(StatusEntry {
            sensitive: is_sensitive_path(&path),
            path,
            status,
        });
    }
    entries
}

fn summaries_from_status(entries: &[StatusEntry]) -> Vec<FileChangeSummary> {
    entries
        .iter()
        .map(|entry| FileChangeSummary {
            path: entry.path.clone(),
            status: entry.status.clone(),
            additions: 0,
            deletions: 0,
            binary: false,
            sensitive: entry.sensitive,
        })
        .collect()
}

fn fill_stats_from_diff(summaries: &mut Vec<FileChangeSummary>, raw: &[u8]) {
    let text = String::from_utf8_lossy(raw);
    // `git status` already classified everything it could see, including the
    // untracked/staged distinction the patch headers cannot express. Only
    // entries discovered from the patch itself need a status derived here.
    let classified_by_status = summaries.len();
    let mut current: Option<usize> = None;
    for line in text.lines() {
        if let Some(path) = diff_path(line) {
            current = summaries.iter().position(|file| file.path == path);
            if current.is_none() {
                summaries.push(FileChangeSummary {
                    sensitive: is_sensitive_path(&path),
                    path,
                    status: "modified".into(),
                    additions: 0,
                    deletions: 0,
                    binary: false,
                });
                current = Some(summaries.len() - 1);
            }
        }
        let Some(index) = current else { continue };
        if index >= classified_by_status {
            if line.starts_with("new file mode ") || line.starts_with("--- /dev/null") {
                summaries[index].status = "added".into();
            } else if line.starts_with("deleted file mode ") || line.starts_with("+++ /dev/null") {
                summaries[index].status = "deleted".into();
            } else if line.starts_with("rename from ") || line.starts_with("rename to ") {
                summaries[index].status = "renamed".into();
            } else if line.starts_with("copy from ") || line.starts_with("copy to ") {
                summaries[index].status = "copied".into();
            }
        }
        if line.starts_with("Binary files") || line.starts_with("GIT binary patch") {
            summaries[index].binary = true;
        } else if line.starts_with('+') && !line.starts_with("+++") {
            summaries[index].additions = summaries[index].additions.saturating_add(1);
        } else if line.starts_with('-') && !line.starts_with("---") {
            summaries[index].deletions = summaries[index].deletions.saturating_add(1);
        }
    }
}

/// Read the reviewed path out of a `diff --git` header.
///
/// Git quotes a side of the header whenever the path holds non-ASCII bytes or
/// characters that need escaping, and it quotes each side independently. A
/// parser that only understands the bare form silently fails to recognise
/// those files, which matters because [`redact_diff`] keys secret suppression
/// off this function.
fn diff_path(line: &str) -> Option<String> {
    let rest = line.strip_prefix("diff --git ")?;
    let (left, right) = split_diff_header(rest)?;
    let left = left.strip_prefix("a/").unwrap_or(&left).to_string();
    let right = right.strip_prefix("b/").unwrap_or(&right).to_string();
    Some(if !right.is_empty() { right } else { left })
}

/// Split the two `a/` and `b/` sides of a header, decoding either side when
/// Git wrote it as a quoted C string.
fn split_diff_header(rest: &str) -> Option<(String, String)> {
    if let Some(quoted) = rest.strip_prefix('"') {
        let (left, remainder) = unquote_git_path(quoted)?;
        let remainder = remainder.strip_prefix(' ')?;
        let right = match remainder.strip_prefix('"') {
            Some(quoted) => unquote_git_path(quoted)?.0,
            None => remainder.to_string(),
        };
        return Some((left, right));
    }
    // An unquoted left side with a quoted right side happens on renames into a
    // non-ASCII name.
    if let Some(index) = rest.find(" \"") {
        let right = unquote_git_path(&rest[index + 2..])?.0;
        return Some((rest[..index].to_string(), right));
    }
    // Both sides bare. A path containing a space stays ambiguous in this form;
    // Git leaves that ambiguity in the header itself.
    let (left, right) = rest.split_once(" b/")?;
    Some((left.to_string(), format!("b/{right}")))
}

/// Decode one Git-quoted path, returning it alongside the rest of the line.
/// Git writes these as C string literals, escaping each non-ASCII *byte* in
/// octal, so decode to bytes first and interpret as UTF-8 afterwards.
fn unquote_git_path(input: &str) -> Option<(String, &str)> {
    let bytes = input.as_bytes();
    let mut decoded = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            // Only ever stopping on an ASCII byte keeps this slice on a
            // character boundary.
            b'"' => {
                return Some((
                    String::from_utf8_lossy(&decoded).into_owned(),
                    &input[index + 1..],
                ))
            }
            b'\\' => {
                let escape = *bytes.get(index + 1)?;
                index += 2;
                match escape {
                    b'a' => decoded.push(0x07),
                    b'b' => decoded.push(0x08),
                    b'f' => decoded.push(0x0c),
                    b'n' => decoded.push(b'\n'),
                    b'r' => decoded.push(b'\r'),
                    b't' => decoded.push(b'\t'),
                    b'v' => decoded.push(0x0b),
                    b'0'..=b'7' => {
                        let mut value = u32::from(escape - b'0');
                        for _ in 0..2 {
                            let digit = bytes
                                .get(index)
                                .copied()
                                .filter(|byte| matches!(byte, b'0'..=b'7'))?;
                            value = value * 8 + u32::from(digit - b'0');
                            index += 1;
                        }
                        decoded.push(u8::try_from(value).ok()?);
                    }
                    other => decoded.push(other),
                }
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    None
}

fn redact_diff(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let mut output = String::new();
    let mut omitted_sensitive_body = false;
    for line in text.lines() {
        if line.starts_with("diff --git ") {
            output.push_str(line);
            output.push('\n');
            // A header this parser cannot read is treated as sensitive. Losing
            // a body is recoverable; printing one that was never checked
            // against the secret list is not.
            omitted_sensitive_body = diff_path(line).is_none_or(|path| is_sensitive_path(&path));
            if omitted_sensitive_body {
                output.push_str("@@ sensitive file omitted @@\n");
            }
            continue;
        }
        if omitted_sensitive_body {
            continue;
        }
        output.push_str(line);
        output.push('\n');
    }
    output
}

/// Head-only clip whose notice sits *outside* the limit, deliberately.
///
/// The flag, not the text, is what this caller acts on, and the notice is a UI
/// affordance rather than a wire cost — so unlike
/// [`crate::bounded::ends_within`] the budget bounds the diff and the notice is
/// added after it. Pinned by `bounded_utf8_marks_truncation`.
fn bounded_utf8(raw: &str, limit: usize) -> (String, bool) {
    if raw.len() <= limit {
        return (raw.to_string(), false);
    }
    let end = crate::bounded::floor_boundary(raw, limit);
    let mut value = raw[..end].to_string();
    value.push_str("\n\n[Diff truncated; file and line counts remain complete.]\n");
    (value, true)
}

async fn change_id(
    root: &Path,
    base_commit: Option<&str>,
    status: &[u8],
    raw_diff: &[u8],
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(root.to_string_lossy().as_bytes());
    hasher.update(base_commit.unwrap_or_default().as_bytes());
    hasher.update(status);
    hasher.update(raw_diff);
    hasher.finalize().to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Git emits `XY <destination>\0<source>\0`, destination first. Reading
    /// the second field instead reports the pre-rename path and then lets the
    /// patch pass append the real one as a phantom second entry.
    #[test]
    fn status_parser_handles_untracked_and_renamed_entries() {
        let raw = b"?? new.txt\0R  new-name.txt\0old.txt\0";
        let entries = parse_status(raw);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].status, "untracked");
        assert_eq!(entries[1].path, "new-name.txt");
        assert_eq!(entries[1].status, "renamed");
    }

    #[test]
    fn a_rename_stays_one_entry_after_patch_stats_are_filled() {
        let mut summaries = summaries_from_status(&parse_status(b"R  new-name.txt\0old.txt\0"));
        fill_stats_from_diff(
            &mut summaries,
            concat!(
                "diff --git a/old.txt b/new-name.txt\n",
                "similarity index 90%\n",
                "rename from old.txt\n",
                "rename to new-name.txt\n",
                "@@ -1 +1 @@\n",
                "-before\n",
                "+after\n",
            )
            .as_bytes(),
        );

        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].path, "new-name.txt");
        assert_eq!(summaries[0].status, "renamed");
        assert_eq!((summaries[0].additions, summaries[0].deletions), (1, 1));
    }

    #[test]
    fn untracked_files_keep_their_status_through_the_patch_pass() {
        let mut summaries = summaries_from_status(&parse_status(b"?? new.txt\0"));
        // The untracked body is synthesised with `git diff --no-index`, whose
        // header claims a new file.
        fill_stats_from_diff(
            &mut summaries,
            concat!(
                "diff --git a/new.txt b/new.txt\n",
                "new file mode 100644\n",
                "--- /dev/null\n",
                "+++ b/new.txt\n",
                "+plain\n",
            )
            .as_bytes(),
        );

        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].status, "untracked");
        assert_eq!(summaries[0].additions, 1);
    }

    #[test]
    fn quoted_headers_resolve_to_the_real_path() {
        assert_eq!(
            diff_path("diff --git \"a/config/.env.caf\\303\\251\" \"b/config/.env.caf\\303\\251\"")
                .as_deref(),
            Some("config/.env.café")
        );
        assert_eq!(
            diff_path("diff --git a/plain.txt \"b/caf\\303\\251.txt\"").as_deref(),
            Some("café.txt")
        );
        assert_eq!(
            diff_path("diff --git \"a/tab\\there\" \"b/tab\\there\"").as_deref(),
            Some("tab\there")
        );
        assert_eq!(
            diff_path("diff --git a/src/lib.rs b/src/lib.rs").as_deref(),
            Some("src/lib.rs")
        );
    }

    /// Git quotes any path holding non-ASCII bytes, so a secret file with an
    /// accented name reaches the redactor in a form the bare parser missed.
    #[test]
    fn sensitive_files_with_quoted_paths_are_still_redacted() {
        let diff = concat!(
            "diff --git a/src/lib.rs b/src/lib.rs\n",
            "@@ -1 +1 @@\n",
            "-old\n",
            "+new\n",
            "diff --git \"a/config/.env.caf\\303\\251\" \"b/config/.env.caf\\303\\251\"\n",
            "@@ -1 +1 @@\n",
            "-API_KEY=supersecret123\n",
            "+API_KEY=rotated_secret_999\n",
        );
        let redacted = redact_diff(diff.as_bytes());
        assert!(!redacted.contains("supersecret123"));
        assert!(!redacted.contains("rotated_secret_999"));
        assert!(redacted.contains("sensitive file omitted"));
        assert!(redacted.contains("+new"));
    }

    /// An unreadable header must suppress the body rather than inherit the
    /// previous section's verdict.
    #[test]
    fn unparseable_headers_are_treated_as_sensitive() {
        let diff = concat!(
            "diff --git a/src/lib.rs b/src/lib.rs\n",
            "@@ -1 +1 @@\n",
            "+safe\n",
            "diff --git nonsense\n",
            "+MAYBE_A_SECRET=1\n",
        );
        let redacted = redact_diff(diff.as_bytes());
        assert!(!redacted.contains("MAYBE_A_SECRET"));
        assert!(redacted.contains("+safe"));
    }

    #[test]
    fn sensitive_sections_are_replaced_without_leaking_body() {
        let diff = "diff --git a/.env b/.env\n+++ b/.env\n@@ -1 +1 @@\n-SECRET\n+NEW_SECRET\ndiff --git a/src/lib.rs b/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\n";
        let redacted = redact_diff(diff.as_bytes());
        assert!(redacted.contains("sensitive file omitted"));
        assert!(!redacted.contains("SECRET"));
        assert!(redacted.contains("src/lib.rs"));
    }

    #[test]
    fn bounded_diff_marks_truncation() {
        let (value, truncated) = bounded_utf8("abcdef", 3);
        assert!(truncated);
        assert!(value.starts_with("abc"));
        assert!(value.contains("Diff truncated"));
    }

    #[test]
    fn worktree_fingerprint_changes_when_same_sized_file_content_changes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("same-size.txt");
        std::fs::write(&path, b"aaaa").unwrap();

        let first = worktree_content_fingerprint(directory.path(), &["same-size.txt".to_string()]);
        std::fs::write(&path, b"bbbb").unwrap();
        let second = worktree_content_fingerprint(directory.path(), &["same-size.txt".to_string()]);

        assert_ne!(first, second);
    }

    #[tokio::test]
    async fn inspection_cache_invalidates_after_a_worktree_edit() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        assert!(std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(root)
            .status()
            .unwrap()
            .success());
        std::fs::write(root.join("tracked.txt"), b"aaaa\n").unwrap();
        assert!(std::process::Command::new("git")
            .args(["add", "--", "tracked.txt"])
            .current_dir(root)
            .status()
            .unwrap()
            .success());
        assert!(std::process::Command::new("git")
            .args([
                "-c",
                "user.name=Zest Test",
                "-c",
                "user.email=zest-test@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "initial",
            ])
            .current_dir(root)
            .status()
            .unwrap()
            .success());

        std::fs::write(root.join("tracked.txt"), b"bbbb\n").unwrap();
        let first = inspect(root, None, None).await.unwrap();
        std::fs::write(root.join("tracked.txt"), b"cccc\n").unwrap();
        let second = inspect(root, None, None).await.unwrap();

        assert!(first.diff.contains("+bbbb"));
        assert!(second.diff.contains("+cccc"));
        assert_ne!(first.change_id, second.change_id);
    }

    #[test]
    fn commit_ids_reject_option_like_values_but_keep_abbreviated_hashes() {
        assert!(is_safe_commit_id("abc123"));
        assert!(is_safe_commit_id(&"a".repeat(40)));
        assert!(is_safe_commit_id(&"b".repeat(64)));
        assert!(!is_safe_commit_id("--output=workspace.patch"));
        assert!(!is_safe_commit_id("not-a-commit"));
    }

    #[test]
    fn pull_request_numbers_reject_zero_and_huge_ids() {
        assert!(is_safe_pr_number(1));
        assert!(is_safe_pr_number(13));
        assert!(!is_safe_pr_number(0));
        assert!(!is_safe_pr_number(1_000_001));
    }

    #[test]
    fn git_refs_reject_ranges_and_flags() {
        assert!(is_safe_git_ref("main"));
        assert!(is_safe_git_ref("feature/pr-chip"));
        assert!(is_safe_git_ref("origin/main"));
        assert!(!is_safe_git_ref("--output=patch"));
        assert!(!is_safe_git_ref("main...HEAD"));
        assert!(!is_safe_git_ref("../etc"));
        assert!(!is_safe_git_ref(""));
    }

    #[test]
    fn unified_diff_snapshot_counts_files_and_redacts_secrets() {
        let diff = concat!(
            "diff --git a/src/lib.rs b/src/lib.rs\n",
            "--- a/src/lib.rs\n",
            "+++ b/src/lib.rs\n",
            "@@ -1 +1 @@\n",
            "-old\n",
            "+new\n",
            "diff --git a/.env b/.env\n",
            "--- a/.env\n",
            "+++ b/.env\n",
            "@@ -1 +1 @@\n",
            "-SECRET=1\n",
            "+SECRET=2\n",
        );
        let snapshot = snapshot_from_unified_diff(
            "pull_request",
            diff.as_bytes(),
            Some("main"),
            Some("topic"),
        );
        assert_eq!(snapshot.repository, "pull_request");
        assert_eq!(snapshot.base_branch.as_deref(), Some("main"));
        assert_eq!(snapshot.branch.as_deref(), Some("topic"));
        assert_eq!(snapshot.changed_files.len(), 2);
        assert_eq!(snapshot.additions, 2);
        assert_eq!(snapshot.deletions, 2);
        assert!(snapshot.diff.contains("+new"));
        assert!(!snapshot.diff.contains("SECRET"));
        assert!(snapshot.diff.contains("sensitive file omitted"));
        assert!(!snapshot.unavailable);
    }

    #[tokio::test]
    async fn merge_base_diff_includes_commits_on_the_topic_branch() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        assert!(std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(root)
            .status()
            .unwrap()
            .success());
        assert!(std::process::Command::new("git")
            .args(["checkout", "-b", "base"])
            .current_dir(root)
            .status()
            .unwrap()
            .success());
        std::fs::write(root.join("tracked.txt"), b"base\n").unwrap();
        assert!(std::process::Command::new("git")
            .args(["add", "--", "tracked.txt"])
            .current_dir(root)
            .status()
            .unwrap()
            .success());
        assert!(std::process::Command::new("git")
            .args([
                "-c",
                "user.name=Zest Test",
                "-c",
                "user.email=zest-test@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "base",
            ])
            .current_dir(root)
            .status()
            .unwrap()
            .success());
        assert!(std::process::Command::new("git")
            .args(["checkout", "-b", "topic"])
            .current_dir(root)
            .status()
            .unwrap()
            .success());
        std::fs::write(root.join("tracked.txt"), b"topic\n").unwrap();
        assert!(std::process::Command::new("git")
            .args(["add", "--", "tracked.txt"])
            .current_dir(root)
            .status()
            .unwrap()
            .success());
        assert!(std::process::Command::new("git")
            .args([
                "-c",
                "user.name=Zest Test",
                "-c",
                "user.email=zest-test@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "topic",
            ])
            .current_dir(root)
            .status()
            .unwrap()
            .success());

        let snapshot = inspect_merge_base(root, "base").await.unwrap();
        assert!(snapshot.diff.contains("-base"));
        assert!(snapshot.diff.contains("+topic"));
        assert!(!snapshot.unavailable);
        assert_eq!(snapshot.base_branch.as_deref(), Some("base"));

        let rejected = inspect_merge_base(root, "--output=patch").await.unwrap();
        assert!(rejected.unavailable);
    }

    #[test]
    fn patch_stats_classify_file_operations() {
        let diff = concat!(
            "diff --git a/added.txt b/added.txt\n",
            "new file mode 100644\n",
            "--- /dev/null\n",
            "+++ b/added.txt\n",
            "+added\n",
            "diff --git a/deleted.txt b/deleted.txt\n",
            "deleted file mode 100644\n",
            "--- a/deleted.txt\n",
            "+++ /dev/null\n",
            "-deleted\n",
            "diff --git a/old.txt b/new.txt\n",
            "similarity index 100%\n",
            "rename from old.txt\n",
            "rename to new.txt\n",
        );
        let mut summaries = Vec::new();
        fill_stats_from_diff(&mut summaries, diff.as_bytes());

        assert_eq!(
            summaries
                .iter()
                .map(|file| (file.path.as_str(), file.status.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("added.txt", "added"),
                ("deleted.txt", "deleted"),
                ("new.txt", "renamed"),
            ]
        );
    }
}
