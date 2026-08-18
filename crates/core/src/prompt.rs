//! System prompt composition: base + project docs + personal skills.

use std::fs;
use std::path::{Path, PathBuf};

use crate::fsutil;
use crate::skills::SkillSet;

/// Default base instructions when a front-end does not supply its own.
///
/// Longer than it looks like it needs to be, for two reasons. The batching line
/// is what actually cashes in concurrent tool execution — the loop can only
/// overlap calls the model chose to issue together. And the edit_file line is
/// worth real money: without it a model reaches for `write_file` and pays for
/// the whole file in output tokens.
pub const DEFAULT_SYSTEM: &str = "\
You are Zest, a coding agent working inside the user's project.

File tools are scoped to the active project directory: list_dir, glob, grep, \
read_file, write_file, and edit_file. Bash requires an explicit `cwd`: use \
`.` for the active project or an absolute path for another project. External \
working directories are approval-gated and shown in the preview. web_search \
reaches public docs and current information.

How to work:
- Read before you answer. Never guess at a file's contents or an API's shape \
when a tool can tell you.
- For project inspection, prefer the scoped grep, glob, and read_file tools. \
They are bounded and cross-platform. Do not use shell searchers such as \
findstr or Select-String for source inspection: on Windows their quoting, \
regex, and encoding behavior is easy to misread. Do not switch to Node just \
to search text.
- Issue independent tool calls together in one turn — they run concurrently. \
Reading three files takes as long as reading one.
- To change an existing file, use edit_file with an exact unique string. Use \
write_file only to create a file or replace one wholesale. Line numbers in \
read_file output are display only; never include them in edit_file arguments.
- Verify with bash, always passing `cwd`. Read-only commands (cargo check, \
cargo test, git status, npm test) run without prompting inside the active \
project, so use them rather than assuming a change compiles. To work on an \
external project, pass the exact absolute directory the user named as `cwd`; do \
not substitute Zest's repository root or rely on `cd` or an inherited shell \
directory. Do not create helper scripts or logs in the active project while \
working on that external project; use bash with that explicit `cwd` and absolute \
log paths, or ask the user to open the external folder as the active project.
- write_file, edit_file, and non-read-only commands ask the user first.

Keep responses focused. State what you verified and what you did not — \
\"it compiles\" and \"it works\" are different claims.";

/// Parent prompt for providers such as Claude Code that own their own model and
/// tool loop. Zest still supplies project context and persistence, but must not
/// describe its local tool registry or invite a second delegation layer.
pub const CLAUDE_CODE_PARENT_SYSTEM: &str = "\
You are the parent coding agent running inside Zest through Claude Code.

Work directly in the active project and use the Claude Code tools available in
this session. Follow the project's AGENTS.md, CLAUDE.md, and other local
instructions. Do not delegate this request to another agent. Read before
changing files, explain important assumptions, and verify meaningful changes
before you finish.

Keep responses focused. State what you verified and what you did not — \
\"it compiles\" and \"it works\" are different claims.";

/// Added only for the desktop parent runtime, which owns a local webview.
pub const LOCAL_BROWSER_SYSTEM: &str = "\\
# Local browser

Use `browser` when a task requires inspecting or operating a live web page. \
Open a URL first, then use `snapshot` to discover accessible controls before \
clicking or typing. Prefer role/name or visible text locators over CSS. \
Click, type, and press are permission-gated; do not enter secrets unless the \
user explicitly supplied them for that page.";

/// Max bytes for `.zest/system.md` (checked before allocating the full body).
pub const MAX_CUSTOM_PROMPT_BYTES: usize = 32 * 1024;

/// Added only when an explicit CLI/ACP worker is configured. External agents
/// are workers, not provider identities, and their changes return for review.
pub const EXTERNAL_DELEGATION_SYSTEM: &str = "\
# External workers

Configured CLI/ACP workers are available through `delegate_feature` for
implementation work and `delegate_external` for compatibility or ad-hoc work.
Use `delegate_feature` when the work belongs on the coordinator board: it
creates a bounded card, uses an isolated worktree, and sends the result through
an independent reviewer before the user can apply it. Use `delegate_external`
only for a self-contained compatibility task with a clear result. ACP file and
terminal requests stay inside the worker workspace, and delegation approval is
the boundary.";

/// Added when the parent runtime registers `ask_user`. The tool is deliberately
/// explicit: the model decides when a real user decision is needed, and the
/// front-end renders the structured request instead of guessing from prose.
pub const INTERACTIVE_QUESTION_SYSTEM: &str = "\
# Asking the user

When you need a user decision, preference, or missing detail before continuing, \
use `ask_user`. Ask one focused question at a time. Use concise `choices` when \
the user should pick from a finite set; omit them for a free-form answer. Do \
not use it for status updates or questions you can answer by inspecting the \
project. The turn pauses until the user answers, then continue from that answer.";

/// Root-level docs pulled into the prompt when present, in priority order.
pub const PROJECT_DOC_FILES: &[&str] = &["AGENTS.md", "CLAUDE.md", "PROJECT_CONTEXT.md"];

/// Total budget across all discovered project docs.
pub const MAX_PROJECT_DOCS_BYTES: usize = 16 * 1024;

pub fn custom_system_path(root: &Path) -> PathBuf {
    root.join(".zest").join("system.md")
}

/// Load custom system prompt. Missing file → empty string. Other I/O / size
/// errors propagate (never silent empty on failure).
pub fn load_custom_system(root: &Path) -> Result<String, String> {
    let path = custom_system_path(root);
    let meta = match fs::metadata(&path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };
    let len = meta.len() as usize;
    if len > MAX_CUSTOM_PROMPT_BYTES {
        return Err(format!(
            "{} is {len} bytes; max is {MAX_CUSTOM_PROMPT_BYTES}",
            path.display()
        ));
    }
    fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))
}

pub fn save_custom_system(root: &Path, content: &str) -> Result<(), String> {
    if content.len() > MAX_CUSTOM_PROMPT_BYTES {
        return Err(format!(
            "custom prompt is {} bytes; max is {MAX_CUSTOM_PROMPT_BYTES}",
            content.len()
        ));
    }
    let path = custom_system_path(root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    fsutil::atomic_write(&path, content.as_bytes())
        .map_err(|e| format!("write {}: {e}", path.display()))
}

/// Compose the full system prompt.
///
/// When `custom` is non-empty it is **authoritative** for identity/persona and is
/// placed first so it overrides conflicting lines in the front-end base prompt
/// (e.g. "You are Zest…"). Skills catalogue follows.
///
/// Everything composed here is stable for a session, which is what makes it
/// worth a cache breakpoint. Anything volatile — see [`env_context`] — belongs
/// after this, not inside it.
pub fn compose_system(base: &str, custom: &str, skills: &SkillSet) -> String {
    compose_system_with_docs(base, custom, "", skills)
}

/// [`compose_system`] plus discovered project docs.
///
/// Ordering is a precedence claim: the user's own `.zest/system.md` wins, then
/// the repo's committed agent docs, then Zest's own rules.
pub fn compose_system_with_docs(
    base: &str,
    custom: &str,
    project_docs: &str,
    skills: &SkillSet,
) -> String {
    let custom = custom.trim();
    let base = base.trim();
    let project_docs = project_docs.trim();
    let mut out = String::new();

    if !custom.is_empty() {
        out.push_str("# Project instructions\n\n");
        out.push_str(custom);
        out.push_str(
            "\n\n(The project instructions above override any conflicting persona \
or identity in the operating rules below.)\n\n# Operating rules\n\n",
        );
        out.push_str(&neutralize_fixed_identity(base));
    } else if !base.is_empty() {
        out.push_str(base);
    }

    if !project_docs.is_empty() {
        out.push_str(
            "\n\n# Project documentation\n\nThe project ships these notes for \
agents working in it. Follow them where they are more specific than the rules \
above; the user's own instructions still win over both.\n\n",
        );
        out.push_str(project_docs);
    }

    let catalogue = skills.catalogue_markdown();
    if !catalogue.is_empty() {
        out.push_str("\n\n# Available skills\n\n");
        out.push_str(&catalogue);
        out.push_str(
            "\n\nUse the `read_skill` tool with a skill's `name` to load full \
instructions when a skill is relevant and its details are not already inlined below.",
        );
    }

    let inline = skills.inline_markdown();
    if !inline.is_empty() {
        out.push_str("\n\n# Skill details\n\n");
        out.push_str(&inline);
    }

    out
}

/// Read root-level agent docs (`AGENTS.md`, `CLAUDE.md`, `PROJECT_CONTEXT.md`).
///
/// These are conventions the project already writes down for whatever agent
/// shows up; not reading them means the harness is the only reader in the room
/// that ignores them. Bounded in total, and each file is labelled so the model
/// can tell instructions from context.
pub fn load_project_docs(root: &Path) -> String {
    let mut out = String::new();
    let mut budget = MAX_PROJECT_DOCS_BYTES;

    for name in PROJECT_DOC_FILES {
        if budget == 0 {
            break;
        }
        let path = root.join(name);
        let Ok(meta) = fs::metadata(&path) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let Ok(body) = fs::read_to_string(&path) else {
            // Unreadable or non-UTF-8: skip it rather than fail the session.
            continue;
        };
        let body = body.trim();
        if body.is_empty() {
            continue;
        }

        let (kept, truncated) = if body.len() > budget {
            (&body[..crate::bounded::floor_boundary(body, budget)], true)
        } else {
            (body, false)
        };
        budget = budget.saturating_sub(kept.len());

        out.push_str(&format!("## {name}\n\n{kept}\n"));
        if truncated {
            out.push_str("\n[truncated — read the file directly for the rest]\n");
        }
        out.push('\n');
    }

    out
}

/// Where the agent is running.
///
/// Kept **out** of [`compose_system`] on purpose: the branch name changes and
/// the cached prompt prefix must not. Front-ends append this after the cached
/// region, or leave it out.
pub fn env_context(root: &Path) -> String {
    let mut out = String::from("# Environment\n\n");
    out.push_str(&format!("Working directory: {}\n", root.display()));
    out.push_str(&format!("Platform: {}\n", std::env::consts::OS));

    match git_branch(root) {
        Some(branch) => out.push_str(&format!("Git repository: yes (branch {branch})\n")),
        None if root.join(".git").exists() => {
            out.push_str("Git repository: yes (branch unknown)\n");
        }
        None => out.push_str("Git repository: no\n"),
    }

    let entries = top_level_entries(root);
    if !entries.is_empty() {
        out.push_str(&format!("Top level: {}\n", entries.join(", ")));
    }
    out
}

/// Read the branch from `.git/HEAD` rather than shelling out to git — this runs
/// on every session start and must not depend on git being installed.
fn git_branch(root: &Path) -> Option<String> {
    let head = fs::read_to_string(root.join(".git").join("HEAD")).ok()?;
    let head = head.trim();
    // Detached HEAD stores a bare sha, which is not a branch name.
    head.strip_prefix("ref: refs/heads/").map(str::to_string)
}

/// A bounded, alphabetical listing so the model knows the shape of the project
/// without spending a `list_dir` call on it.
fn top_level_entries(root: &Path) -> Vec<String> {
    const MAX_ENTRIES: usize = 40;
    let Ok(dir) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut names: Vec<String> = dir
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || name == "node_modules" || name == "target" {
                return None;
            }
            let suffix = match e.file_type() {
                Ok(t) if t.is_dir() => "/",
                _ => "",
            };
            Some(format!("{name}{suffix}"))
        })
        .collect();
    names.sort();
    if names.len() > MAX_ENTRIES {
        let extra = names.len() - MAX_ENTRIES;
        names.truncate(MAX_ENTRIES);
        names.push(format!("… (+{extra} more)"));
    }
    names
}

/// Soften a hardcoded "You are Zest…" opener so project custom identity can win.
fn neutralize_fixed_identity(base: &str) -> String {
    let trimmed = base.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("you are zest") {
        // Drop the first sentence; keep tooling / behavior rules.
        if let Some(rest) = trimmed.split_once(". ").map(|(_, r)| r) {
            return format!("You are a coding agent in the user's project. {rest}");
        }
    }
    trimmed.to_string()
}

/// Unicode-safe truncation for composed-prompt previews (char-based, not bytes).
pub fn truncate_chars(s: &str, max_chars: usize) -> String {
    let count = s.chars().count();
    if count <= max_chars {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max_chars).collect();
    format!("{truncated}…\n\n(truncated — {count} chars total)")
}

/// Load custom + project docs + user skills and compose against `base`.
pub fn compose_for_project(base: &str, root: &Path) -> Result<(String, SkillSet), String> {
    let custom = load_custom_system(root)?;
    let docs = load_project_docs(root);
    let skills = SkillSet::discover();
    let system = compose_system_with_docs(base, &custom, &docs, &skills);
    Ok((system, skills))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::parse_skill_markdown;
    use std::path::Path;

    #[test]
    fn compose_order_base_custom_skills() {
        let mut skills = SkillSet::default();
        let skill = parse_skill_markdown(
            "---\nname: fmt\ndescription: Format code\n---\n\nDo it right.\n",
            Path::new("/x/fmt/SKILL.md"),
        )
        .unwrap();
        skills.insert(skill);

        let composed = compose_system("BASE tooling", "CUSTOM LAYER", &skills);
        let custom_at = composed.find("CUSTOM LAYER").unwrap();
        let base_at = composed.find("BASE tooling").unwrap();
        let skills_at = composed.find("Available skills").unwrap();
        assert!(custom_at < base_at, "custom must precede base");
        assert!(base_at < skills_at);
        assert!(composed.contains("`fmt`: Format code"));
        assert!(composed.contains("Do it right."));
    }

    #[test]
    fn custom_identity_overrides_you_are_zest() {
        let skills = SkillSet::default();
        let composed = compose_system(
            "You are Zest, a coding agent. You have tools.",
            "You are jennie of blackpink",
            &skills,
        );
        let jennie = composed.find("You are jennie of blackpink").unwrap();
        let zest = composed.find("You are Zest");
        assert!(zest.is_none(), "fixed Zest identity should be neutralized");
        assert!(composed[jennie..].contains("You are a coding agent"));
        assert!(composed.contains("override"));
    }

    #[test]
    fn truncate_chars_is_multibyte_safe() {
        // Each emoji is one char but multiple UTF-8 bytes.
        let s = "😀😁😂😃😄😅😆😇😈";
        let out = truncate_chars(s, 3);
        assert!(out.starts_with("😀😁😂"));
        assert!(out.contains("truncated"));
        // Must not panic or split a codepoint.
        assert!(std::str::from_utf8(out.as_bytes()).is_ok());
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("zest-prompt-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn project_docs_are_discovered_and_labelled() {
        let dir = scratch("docs");
        fs::write(dir.join("AGENTS.md"), "Always use tabs.").unwrap();
        fs::write(dir.join("PROJECT_CONTEXT.md"), "This is a widget factory.").unwrap();
        // Not on the list — must not be swept in.
        fs::write(dir.join("NOTES.md"), "secret plans").unwrap();

        let docs = load_project_docs(&dir);
        assert!(docs.contains("## AGENTS.md"), "{docs}");
        assert!(docs.contains("Always use tabs."), "{docs}");
        assert!(docs.contains("## PROJECT_CONTEXT.md"), "{docs}");
        assert!(!docs.contains("secret plans"), "{docs}");
    }

    #[test]
    fn missing_and_empty_docs_are_simply_absent() {
        let dir = scratch("nodocs");
        assert!(load_project_docs(&dir).is_empty());
        fs::write(dir.join("AGENTS.md"), "   \n\n ").unwrap();
        assert!(
            load_project_docs(&dir).is_empty(),
            "whitespace is not content"
        );
    }

    #[test]
    fn project_docs_are_bounded_in_total() {
        let dir = scratch("bigdocs");
        fs::write(
            dir.join("AGENTS.md"),
            "a".repeat(MAX_PROJECT_DOCS_BYTES * 2),
        )
        .unwrap();
        fs::write(dir.join("CLAUDE.md"), "b".repeat(1000)).unwrap();
        let docs = load_project_docs(&dir);
        assert!(docs.contains("truncated"), "{}", &docs[..80]);
        // The budget is shared, so the second file cannot smuggle more past it.
        assert!(!docs.contains('b'), "budget was not shared across files");
        assert!(docs.len() < MAX_PROJECT_DOCS_BYTES * 2);
    }

    #[test]
    fn truncating_docs_never_splits_a_codepoint() {
        let dir = scratch("utf8docs");
        fs::write(dir.join("AGENTS.md"), "é".repeat(MAX_PROJECT_DOCS_BYTES)).unwrap();
        let docs = load_project_docs(&dir);
        assert!(std::str::from_utf8(docs.as_bytes()).is_ok());
    }

    #[test]
    fn user_instructions_outrank_project_docs_which_outrank_base() {
        let skills = SkillSet::default();
        let composed = compose_system_with_docs(
            "BASE RULES",
            "CUSTOM LAYER",
            "## AGENTS.md\n\nDOC RULES",
            &skills,
        );
        let custom_at = composed.find("CUSTOM LAYER").unwrap();
        let base_at = composed.find("BASE RULES").unwrap();
        let docs_at = composed.find("DOC RULES").unwrap();
        assert!(custom_at < base_at);
        assert!(base_at < docs_at);
        assert!(composed.contains("user's own instructions still win"));
    }

    #[test]
    fn compose_system_without_docs_is_unchanged() {
        let skills = SkillSet::default();
        assert_eq!(
            compose_system("BASE", "", &skills),
            compose_system_with_docs("BASE", "", "", &skills)
        );
        assert!(!compose_system("BASE", "", &skills).contains("Project documentation"));
    }

    #[test]
    fn default_system_prefers_scoped_inspection_tools() {
        assert!(DEFAULT_SYSTEM.contains("scoped grep, glob, and read_file tools"));
        assert!(DEFAULT_SYSTEM.contains("Bash requires an explicit `cwd`"));
        assert!(DEFAULT_SYSTEM.contains("exact absolute directory the user named"));
        assert!(DEFAULT_SYSTEM.contains("Do not create helper scripts or logs"));
        assert!(DEFAULT_SYSTEM.contains("or rely on `cd`"));
        assert!(DEFAULT_SYSTEM.contains("findstr or Select-String"));
        assert!(DEFAULT_SYSTEM.contains("Do not switch to Node just"));
    }

    #[test]
    fn env_context_reports_directory_platform_and_git() {
        let dir = scratch("env");
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("Cargo.toml"), "").unwrap();

        let env = env_context(&dir);
        assert!(env.contains("Working directory:"), "{env}");
        assert!(env.contains(std::env::consts::OS), "{env}");
        assert!(env.contains("Git repository: no"), "{env}");
        assert!(env.contains("src/"), "{env}");
        assert!(env.contains("Cargo.toml"), "{env}");
    }

    #[test]
    fn env_context_reads_the_branch_without_running_git() {
        let dir = scratch("branch");
        fs::create_dir_all(dir.join(".git")).unwrap();
        fs::write(dir.join(".git").join("HEAD"), "ref: refs/heads/feature/x\n").unwrap();
        assert!(env_context(&dir).contains("branch feature/x"));

        // Detached HEAD stores a bare sha — not a branch, and not reported as one.
        fs::write(dir.join(".git").join("HEAD"), "abc123def\n").unwrap();
        let env = env_context(&dir);
        assert!(env.contains("branch unknown"), "{env}");
        assert!(!env.contains("abc123def"), "{env}");
    }

    /// The environment block must not be inside the region a cache breakpoint
    /// covers, or the branch changing would cost a full cache miss.
    #[test]
    fn env_context_is_not_part_of_the_composed_prefix() {
        let dir = scratch("cachesafe");
        fs::create_dir_all(dir.join(".git")).unwrap();
        fs::write(dir.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();
        let (composed, _) = compose_for_project("BASE", &dir).unwrap();
        assert!(
            !composed.contains("# Environment"),
            "compose_for_project must leave env to the caller"
        );
    }

    #[test]
    fn load_custom_rejects_oversized() {
        let dir = std::env::temp_dir().join(format!(
            "zest-prompt-big-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".zest")).unwrap();
        let big = "x".repeat(MAX_CUSTOM_PROMPT_BYTES + 1);
        fs::write(dir.join(".zest").join("system.md"), &big).unwrap();
        let err = load_custom_system(&dir).unwrap_err();
        assert!(err.contains("max"), "{err}");
    }
}
