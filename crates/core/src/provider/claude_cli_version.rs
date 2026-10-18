//! Which Claude Code CLI is installed, so the picker can say ahead of time
//! which explicit models it is too old to run.
//!
//! The API refuses some explicit ids from an older client, but only once a
//! turn is sent: "Claude Code 2.1.220 does not support this model; version
//! 2.1.280 or newer is required". `claude --version` answers offline and spends
//! no quota, so the catalogue asks it instead.
//!
//! The answer is cached for the life of the process, keyed by the configured
//! command. On Windows the CLI takes over a second to print its version, so a
//! provider starts the probe in the background as soon as it is built
//! ([`warm`]) and a catalogue that asks first ([`installed`]) waits only for
//! whatever is left. A CLI updated while Zest runs is seen after a restart.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock};

/// A Claude Code release, compared by `major.minor.patch` only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CliVersion {
    major: u32,
    minor: u32,
    patch: u32,
}

impl CliVersion {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// The installed version in `claude --version` output.
    ///
    /// The CLI prints `2.1.220 (Claude Code)`. A line that names Claude Code
    /// wins over any other, so an update notice that mentions the newer release
    /// first is not mistaken for the installed one. Colour codes, a `v` prefix
    /// and a pre-release or build suffix are tolerated; the suffix is ignored,
    /// so `2.1.280-beta.1` compares as `2.1.280`. That errs towards calling a
    /// model runnable, which is the cheap mistake: the API still has the final
    /// word when the turn is sent.
    pub fn parse(output: &str) -> Option<Self> {
        let output = strip_ansi(output);
        let lines = || {
            output
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
        };
        lines()
            .filter(|line| line.contains("Claude Code"))
            .chain(lines())
            .find_map(first_version)
    }
}

impl fmt::Display for CliVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// The first `major.minor.patch` token on a line. Two parts is not enough and
/// four is something else (an address, a Windows build number).
fn first_version(line: &str) -> Option<CliVersion> {
    line.split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | ',' | ':' | ';'))
        .find_map(|token| {
            let token = token.strip_prefix(['v', 'V']).unwrap_or(token);
            let core = token
                .split(['-', '+'])
                .next()
                .unwrap_or_default()
                .trim_end_matches('.');
            let parts: Vec<&str> = core.split('.').collect();
            if parts.len() != 3
                || parts
                    .iter()
                    .any(|part| part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()))
            {
                return None;
            }
            Some(CliVersion::new(
                parts[0].parse().ok()?,
                parts[1].parse().ok()?,
                parts[2].parse().ok()?,
            ))
        })
}

/// Drop ANSI escape sequences (`ESC [ ... final`), which a CLI can emit even
/// when its output is a pipe.
fn strip_ansi(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        if chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if ('@'..='~').contains(&next) {
                    break;
                }
            }
        }
    }
    out
}

type Slot = Arc<OnceLock<Option<CliVersion>>>;

/// One slot per command. A slot is filled once; a caller that arrives while the
/// probe is running blocks on it instead of starting a second process.
fn slot(command: &str) -> Slot {
    static SLOTS: OnceLock<Mutex<HashMap<String, Slot>>> = OnceLock::new();
    let mut slots = SLOTS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    slots.entry(command.to_string()).or_default().clone()
}

/// The installed CLI's version, probing it on first use. `None` when the CLI
/// is missing, slow, or prints something unrecognisable — every model is then
/// treated as runnable.
pub fn installed(command: &str) -> Option<CliVersion> {
    *slot(command).get_or_init(|| probe(command))
}

/// Start the probe without waiting for it, so the first catalogue read does not
/// pay for the CLI's start-up.
pub fn warm(command: &str) {
    let slot = slot(command);
    if slot.get().is_some() {
        return;
    }
    let command = command.to_string();
    let _ = std::thread::Builder::new()
        .name("claude-cli-version".into())
        .spawn(move || {
            slot.get_or_init(|| probe(&command));
        });
}

/// Fill the cache for `command` without running anything.
#[cfg(test)]
pub(crate) fn seed(command: &str, version: Option<CliVersion>) {
    let _ = slot(command).set(version);
}

/// Unit tests never start a real CLI: what is installed on the machine running
/// them must not change their result. Tests that need a version [`seed`] one.
#[cfg(test)]
fn probe(_command: &str) -> Option<CliVersion> {
    None
}

#[cfg(not(test))]
fn probe(command: &str) -> Option<CliVersion> {
    run_version(command)
}

/// Run `<command> --version` with a time bound and parse what it prints.
fn run_version(command: &str) -> Option<CliVersion> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    // The same bound the Settings check gives `--version`.
    const TIMEOUT: Duration = Duration::from_secs(8);

    let mut process = Command::new(crate::tools::external_agent::resolve_program(command));
    process
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    crate::tools::external_agent::prepare_sync_external_command(&mut process);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: a GUI process spawning a console program would
        // otherwise flash a console window.
        process.creation_flags(0x0800_0000);
    }

    let mut child = process.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });

    let deadline = Instant::now() + TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(25));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let text = reader.join().ok()?;
    if !status.success() {
        return None;
    }
    CliVersion::parse(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(raw: &str) -> Option<String> {
        CliVersion::parse(raw).map(|version| version.to_string())
    }

    #[test]
    fn reads_the_version_claude_prints() {
        assert_eq!(parse("2.1.220 (Claude Code)\n").as_deref(), Some("2.1.220"));
        assert_eq!(
            parse("2.1.220 (Claude Code)\r\n").as_deref(),
            Some("2.1.220")
        );
        assert_eq!(parse("  2.1.280  ").as_deref(), Some("2.1.280"));
    }

    #[test]
    fn tolerates_odd_version_output() {
        assert_eq!(parse("v2.1.251").as_deref(), Some("2.1.251"));
        assert_eq!(parse("Claude Code v2.1.300").as_deref(), Some("2.1.300"));
        assert_eq!(
            parse("2.1.280-beta.1 (Claude Code)").as_deref(),
            Some("2.1.280")
        );
        assert_eq!(parse("2.1.280+build.7").as_deref(), Some("2.1.280"));
        assert_eq!(
            parse("\u{1b}[1m2.1.220\u{1b}[0m (Claude Code)").as_deref(),
            Some("2.1.220")
        );
        assert_eq!(
            parse("(node:4120) ExperimentalWarning: something\n2.1.220 (Claude Code)").as_deref(),
            Some("2.1.220")
        );
    }

    #[test]
    fn the_line_naming_claude_code_beats_an_update_notice() {
        let raw = "Update available: 2.1.290\n2.1.220 (Claude Code)\n";
        assert_eq!(parse(raw).as_deref(), Some("2.1.220"));
    }

    #[test]
    fn rejects_output_with_no_version() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("command not found: claude"), None);
        assert_eq!(parse("2.1 (Claude Code)"), None);
        assert_eq!(parse("listening on 127.0.0.1"), None);
        assert_eq!(parse("2.x.220"), None);
        assert_eq!(parse("99999999999.1.1"), None);
    }

    #[test]
    fn versions_compare_numerically_not_as_text() {
        let v = |raw: &str| CliVersion::parse(raw).unwrap();
        assert!(v("2.1.220") < v("2.1.280"));
        assert!(v("2.1.99") < v("2.1.100"));
        assert!(v("2.1.300") < v("2.2.0"));
        assert!(v("2.10.0") > v("2.9.999"));
        assert_eq!(v("v2.1.280"), CliVersion::new(2, 1, 280));
    }

    /// Opt-in: runs whatever `claude` is on this machine's PATH. Offline, and
    /// spends no quota.
    #[test]
    #[ignore = "runs the installed `claude --version`"]
    fn live_reads_the_installed_cli_version() {
        let version = run_version("claude").expect("`claude --version` answered");
        eprintln!("installed Claude Code: {version}");
        assert!(version >= CliVersion::new(1, 0, 0));
    }

    #[test]
    fn a_seeded_version_is_returned_without_probing() {
        seed("zest-test-seeded-claude", Some(CliVersion::new(2, 1, 220)));
        assert_eq!(
            installed("zest-test-seeded-claude"),
            Some(CliVersion::new(2, 1, 220))
        );
        // Under test the probe never runs, so an unseeded command is unknown.
        assert_eq!(installed("zest-test-unseeded-claude"), None);
    }
}
