//! Claude Code version parsing and passive catalogue metadata.
//!
//! A project may supply any executable as its Claude command. Reading a model
//! catalogue must not run it, inherit credentials into a child, or wait for
//! subprocess output. Only recognizable installed-file metadata is used;
//! opaque commands stay unknown and the provider's turn error is authoritative.

#[cfg(test)]
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fmt;
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;
#[cfg(test)]
use std::sync::{Mutex, OnceLock};

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

#[cfg(test)]
fn seeded_versions() -> &'static Mutex<HashMap<String, Option<CliVersion>>> {
    static VERSIONS: OnceLock<Mutex<HashMap<String, Option<CliVersion>>>> = OnceLock::new();
    VERSIONS.get_or_init(Default::default)
}

/// Passive lookup only. No configured command is ever executed here.
pub fn installed(command: &str) -> Option<CliVersion> {
    #[cfg(test)]
    if let Some(version) = seeded_versions().lock().ok()?.get(command).copied() {
        return version;
    }

    if command != "claude" {
        return None;
    }
    let search = std::env::var_os("PATH")?;
    let extensions = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    installed_from_path(command, &search, &extensions)
}

fn installed_from_path(command: &str, search: &OsStr, extensions: &str) -> Option<CliVersion> {
    if command != "claude" {
        return None;
    }
    let names = candidate_names(extensions)?;
    for dir in std::env::split_paths(search) {
        // An earlier cwd-dependent entry could win at launch; do not guess past it.
        if !dir.is_absolute() {
            return None;
        }
        for name in &names {
            let candidate = dir.join(name);
            match fs::symlink_metadata(&candidate) {
                Ok(metadata) if metadata.is_dir() => continue,
                Ok(_) => return candidate_version(&candidate),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => return None,
            }
        }
    }
    None
}

fn candidate_names(_extensions: &str) -> Option<Vec<String>> {
    #[cfg(windows)]
    {
        if _extensions.len() > 4096 {
            return None;
        }
        let mut names = Vec::new();
        for extension in _extensions
            .split(';')
            .map(str::trim)
            .filter(|e| !e.is_empty())
        {
            let suffix = extension.strip_prefix('.')?;
            if suffix.is_empty()
                || suffix.len() > 16
                || !suffix.bytes().all(|b| b.is_ascii_alphanumeric())
            {
                return None;
            }
            names.push(format!("claude{extension}"));
        }
        Some(names)
    }
    #[cfg(not(windows))]
    Some(vec!["claude".into()])
}

fn candidate_version(candidate: &Path) -> Option<CliVersion> {
    // Never resolve a relative filename through cwd, including in Win32 APIs.
    if !candidate.is_absolute() {
        return None;
    }
    let canonical = fs::canonicalize(candidate).ok()?;
    if !fs::metadata(&canonical).ok()?.is_file() {
        return None;
    }
    if let Some(version) = npm_version(&canonical).or_else(|| native_layout_version(&canonical)) {
        return Some(version);
    }
    #[cfg(windows)]
    return windows_resource::version(&canonical);
    #[cfg(not(windows))]
    None
}

/// Exact releases only for file metadata; keep CLI-output parsing permissive.
fn exact_version(raw: &str) -> Option<CliVersion> {
    let version = first_version(raw)?;
    (version.to_string() == raw).then_some(version)
}

fn bounded_regular_file(path: &Path, max: usize) -> Option<Vec<u8>> {
    if !fs::symlink_metadata(path).ok()?.is_file() {
        return None;
    }
    let file = File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() > max as u64 {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(max as u64 + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() <= max).then_some(bytes)
}

// Recognize one known npm cmd-shim template, not arbitrary batch instructions.
const NPM_CMD_LAUNCHER: &str = r#"@ECHO off
GOTO start
:find_dp0
SET dp0=%~dp0
EXIT /b
:start
SETLOCAL
CALL :find_dp0

IF EXIST "%dp0%\node.exe" (
  SET "_prog=%dp0%\node.exe"
) ELSE (
  SET "_prog=node"
  SET PATHEXT=%PATHEXT:;.JS;=;%
)

endLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & "%_prog%"  "%dp0%\node_modules\@anthropic-ai\claude-code\cli.js" %*
"#;

fn npm_version(candidate: &Path) -> Option<CliVersion> {
    let parent = candidate.parent()?;
    let package = if candidate.file_name()? == "cli.js"
        && parent.file_name()? == "claude-code"
        && parent.parent()?.file_name()? == "@anthropic-ai"
    {
        parent.to_path_buf()
    } else {
        if !candidate
            .file_name()?
            .to_str()?
            .eq_ignore_ascii_case("claude.cmd")
        {
            return None;
        }
        let shim = String::from_utf8(bounded_regular_file(candidate, 16 * 1024)?).ok()?;
        if shim.replace("\r\n", "\n") != NPM_CMD_LAUNCHER {
            return None;
        }
        let package = parent.join("node_modules/@anthropic-ai/claude-code");
        if !package.join("cli.js").is_file() {
            return None;
        }
        package
    };
    #[derive(serde::Deserialize)]
    struct Package {
        name: String,
        version: String,
    }
    let bytes = bounded_regular_file(&package.join("package.json"), 64 * 1024)?;
    let metadata: Package = serde_json::from_slice(&bytes).ok()?;
    (metadata.name == "@anthropic-ai/claude-code")
        .then(|| exact_version(&metadata.version))
        .flatten()
}

fn native_layout_version(candidate: &Path) -> Option<CliVersion> {
    let filename = candidate.file_name()?.to_str()?;
    let parent = candidate.parent()?;
    let (versions, release) = if filename == "claude" || filename.eq_ignore_ascii_case("claude.exe")
    {
        (parent.parent()?, parent.file_name()?.to_str()?)
    } else {
        (parent, filename.strip_suffix(".exe").unwrap_or(filename))
    };
    if versions.file_name()? != "versions" || versions.parent()?.file_name()? != "claude" {
        return None;
    }
    let version = exact_version(release)?;
    let mut magic = [0; 4];
    File::open(candidate).ok()?.read_exact(&mut magic).ok()?;
    let native = if cfg!(windows) {
        magic.starts_with(b"MZ")
    } else if cfg!(target_os = "macos") {
        matches!(
            magic,
            [0xcf, 0xfa, 0xed, 0xfe] | [0xce, 0xfa, 0xed, 0xfe] | [0xca, 0xfe, 0xba, 0xbe]
        )
    } else {
        magic == *b"\x7fELF"
    };
    native.then_some(version)
}

#[cfg(windows)]
mod windows_resource {
    use super::*;
    use std::ffi::c_void;
    use std::mem::size_of;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW, VFT_APP, VS_FIXEDFILEINFO,
    };

    const MAX_RESOURCE_BYTES: u32 = 1024 * 1024;

    /// `VerQueryValueW` lengths are bytes except string lengths, which are UTF-16 units.
    fn query<'a>(buffer: &'a [u8], key: &str, unit_bytes: usize) -> Option<&'a [u8]> {
        let key: Vec<u16> = key.encode_utf16().chain(Some(0)).collect();
        let mut pointer: *mut c_void = std::ptr::null_mut();
        let mut length = 0;
        // SAFETY: buffer came from a successful GetFileVersionInfoW call; key is terminated.
        if unsafe {
            VerQueryValueW(
                buffer.as_ptr().cast(),
                key.as_ptr(),
                &mut pointer,
                &mut length,
            )
        } == 0
        {
            return None;
        }
        let bytes = (length as usize).checked_mul(unit_bytes)?;
        checked_range(buffer, pointer as usize, bytes)
    }

    pub(super) fn checked_range(buffer: &[u8], address: usize, length: usize) -> Option<&[u8]> {
        if length == 0 {
            return None;
        }
        let offset = address.checked_sub(buffer.as_ptr() as usize)?;
        buffer.get(offset..offset.checked_add(length)?)
    }

    pub(super) fn version(path: &Path) -> Option<CliVersion> {
        if !path.is_absolute() {
            return None;
        }
        let canonical = fs::canonicalize(path).ok()?;
        if !fs::metadata(&canonical).ok()?.is_file() {
            return None;
        }
        let filename: Vec<u16> = canonical.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut ignored = 0;
        // SAFETY: filename is an absolute canonical path with a terminating NUL.
        let size = unsafe { GetFileVersionInfoSizeW(filename.as_ptr(), &mut ignored) };
        if size == 0 || size > MAX_RESOURCE_BYTES {
            return None;
        }
        let mut buffer = vec![0u8; size as usize];
        // SAFETY: the writable buffer has exactly the API-requested, bounded size.
        if unsafe { GetFileVersionInfoW(filename.as_ptr(), 0, size, buffer.as_mut_ptr().cast()) }
            == 0
        {
            return None;
        }
        let root = query(&buffer, "\\", 1)?;
        if root.len() < size_of::<VS_FIXEDFILEINFO>() {
            return None;
        }
        // SAFETY: query checked the full range; a byte buffer need not be struct-aligned.
        let fixed = unsafe { std::ptr::read_unaligned(root.as_ptr().cast::<VS_FIXEDFILEINFO>()) };
        if fixed.dwSignature != 0xfeef04bd
            || fixed.dwStrucVersion != 0x00010000
            || fixed.dwFileType != VFT_APP as u32
            || (fixed.dwFileVersionMS | fixed.dwFileVersionLS) == 0
        {
            return None;
        }
        let translations = query(&buffer, "\\VarFileInfo\\Translation", 1)?;
        if translations.len() > 256 || translations.len() % 4 != 0 {
            return None;
        }
        for translation in translations.chunks_exact(4) {
            let language = u16::from_le_bytes([translation[0], translation[1]]);
            let codepage = u16::from_le_bytes([translation[2], translation[3]]);
            let key = format!("\\StringFileInfo\\{language:04x}{codepage:04x}\\ProductName");
            let Some(product) = query(&buffer, &key, 2).filter(|value| value.len() <= 128) else {
                continue;
            };
            let units: Vec<u16> = product
                .chunks_exact(2)
                .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
                .collect();
            if units.last() == Some(&0)
                && String::from_utf16(&units[..units.len() - 1])
                    .ok()
                    .as_deref()
                    == Some("Claude Code")
            {
                return Some(CliVersion::new(
                    fixed.dwFileVersionMS >> 16,
                    fixed.dwFileVersionMS & 0xffff,
                    fixed.dwFileVersionLS >> 16,
                ));
            }
        }
        None
    }
}

/// Test fixtures can supply metadata without executing a CLI.
#[cfg(test)]
pub(crate) fn seed(command: &str, version: Option<CliVersion>) {
    seeded_versions()
        .lock()
        .unwrap()
        .insert(command.into(), version);
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

    #[test]
    fn passive_metadata_is_unknown_for_custom_and_path_commands() {
        for command in [
            "zest-untrusted-command",
            "./project-claude",
            "C:/project/claude.cmd",
        ] {
            assert_eq!(installed(command), None);
        }
    }

    #[test]
    fn a_seeded_version_is_returned_without_probing() {
        seed("zest-test-seeded-claude", Some(CliVersion::new(2, 1, 220)));
        assert_eq!(
            installed("zest-test-seeded-claude"),
            Some(CliVersion::new(2, 1, 220))
        );
        // An unseeded custom command is unknown in production as well.
        assert_eq!(installed("zest-test-unseeded-claude"), None);
    }

    const EXTENSIONS: &str = ".COM;.EXE;.BAT;.CMD";

    fn lookup(command: &str, dirs: &[&Path]) -> Option<CliVersion> {
        let search = std::env::join_paths(dirs).unwrap();
        installed_from_path(command, &search, EXTENSIONS)
    }

    fn npm_fixture(dir: &Path, manifest: &[u8]) -> std::path::PathBuf {
        let package = dir.join("node_modules/@anthropic-ai/claude-code");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(package.join("package.json"), manifest).unwrap();
        std::fs::write(package.join("cli.js"), "// never executed\n").unwrap();
        #[cfg(windows)]
        let command = {
            let command = dir.join("claude.cmd");
            let shim = "@ECHO off\nGOTO start\n:find_dp0\nSET dp0=%~dp0\nEXIT /b\n:start\nSETLOCAL\nCALL :find_dp0\n\nIF EXIST \"%dp0%\\node.exe\" (\n  SET \"_prog=%dp0%\\node.exe\"\n) ELSE (\n  SET \"_prog=node\"\n  SET PATHEXT=%PATHEXT:;.JS;=;%\n)\n\nendLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\"  \"%dp0%\\node_modules\\@anthropic-ai\\claude-code\\cli.js\" %*\n";
            std::fs::write(&command, shim.replace('\n', "\r\n")).unwrap();
            command
        };
        #[cfg(unix)]
        let command = {
            let command = dir.join("claude");
            if command.symlink_metadata().is_ok() {
                std::fs::remove_file(&command).unwrap();
            }
            std::os::unix::fs::symlink(package.join("cli.js"), &command).unwrap();
            command
        };
        command
    }

    #[test]
    fn production_lookup_reads_npm_metadata_without_running_the_shim() {
        let root = tempfile::tempdir().unwrap();
        npm_fixture(
            root.path(),
            br#"{"name":"@anthropic-ai/claude-code","version":"2.1.220"}"#,
        );
        assert_eq!(
            lookup("claude", &[root.path()]),
            Some(CliVersion::new(2, 1, 220))
        );
        assert_eq!(lookup("custom-claude", &[root.path()]), None);
        assert_eq!(lookup("./claude", &[root.path()]), None);
        assert_eq!(
            lookup(root.path().join("claude").to_str().unwrap(), &[root.path()]),
            None
        );
    }

    #[test]
    fn npm_metadata_must_name_claude_and_an_exact_release() {
        let root = tempfile::tempdir().unwrap();
        for manifest in [
            &b"not json"[..],
            br#"{"name":"other-package","version":"2.1.220"}"#,
            br#"{"name":"@anthropic-ai/claude-code","version":"v2.1.220"}"#,
            br#"{"name":"@anthropic-ai/claude-code","version":"2.1.220-beta.1"}"#,
            br#"{"name":"@anthropic-ai/claude-code","version":"02.1.220"}"#,
            br#"{"name":"@anthropic-ai/claude-code","version":220}"#,
            br#"{"name":"@anthropic-ai/claude-code"}"#,
        ] {
            npm_fixture(root.path(), manifest);
            assert_eq!(lookup("claude", &[root.path()]), None, "{manifest:?}");
        }
        let mut oversized = br#"{"name":"@anthropic-ai/claude-code","version":"2.1.220"}"#.to_vec();
        oversized.resize(65 * 1024, b' ');
        npm_fixture(root.path(), &oversized);
        assert_eq!(lookup("claude", &[root.path()]), None);
    }

    #[test]
    fn earlier_relative_or_empty_path_entries_make_the_version_unknown() {
        let root = tempfile::tempdir().unwrap();
        npm_fixture(
            root.path(),
            br#"{"name":"@anthropic-ai/claude-code","version":"2.1.251"}"#,
        );
        assert_eq!(
            lookup("claude", &[Path::new("."), Path::new("relative/bin")]),
            None
        );
        assert_eq!(lookup("claude", &[Path::new("."), root.path()]), None);
        assert_eq!(lookup("claude", &[Path::new(""), root.path()]), None);
        assert_eq!(
            lookup("claude", &[root.path(), Path::new(".")]),
            Some(CliVersion::new(2, 1, 251))
        );
        assert_eq!(
            lookup("claude", &[root.path(), Path::new("")]),
            Some(CliVersion::new(2, 1, 251))
        );
    }

    #[test]
    fn npm_metadata_is_not_attributed_to_an_opaque_wrapper_comment() {
        let root = tempfile::tempdir().unwrap();
        let command = npm_fixture(
            root.path(),
            br#"{"name":"@anthropic-ai/claude-code","version":"2.1.220"}"#,
        );
        #[cfg(unix)]
        std::fs::remove_file(&command).unwrap();
        let wrapper = if cfg!(windows) {
            "@ECHO off\r\nREM \"%dp0%\\node_modules\\@anthropic-ai\\claude-code\\cli.js\"\r\nC:\\Other\\claude.exe %*\r\n"
        } else {
            "#!/bin/sh\n# \"$basedir/node_modules/@anthropic-ai/claude-code/cli.js\"\nexec /other/claude \"$@\"\n"
        };
        std::fs::write(&command, wrapper).unwrap();
        assert_eq!(lookup("claude", &[root.path()]), None);
    }

    #[cfg(windows)]
    #[test]
    fn npm_cmd_launcher_must_match_the_entire_supported_template() {
        let root = tempfile::tempdir().unwrap();
        let command = npm_fixture(
            root.path(),
            br#"{"name":"@anthropic-ai/claude-code","version":"2.1.220"}"#,
        );
        let template = std::fs::read_to_string(&command).unwrap();
        // Both npm line endings are recognized without parsing arbitrary batch syntax.
        std::fs::write(&command, template.replace("\r\n", "\n")).unwrap();
        assert_eq!(
            lookup("claude", &[root.path()]),
            Some(CliVersion::new(2, 1, 220))
        );
        for wrapper in [
            format!("{template}C:\\Other\\claude.exe %*\r\n"),
            template.replace("SET \"_prog=node\"", "SET \"_prog=C:\\Other\\claude.exe\""),
        ] {
            std::fs::write(&command, wrapper).unwrap();
            assert_eq!(lookup("claude", &[root.path()]), None);
        }
    }

    #[test]
    fn the_first_opaque_candidate_is_unknown_and_its_script_never_runs() {
        let first = tempfile::tempdir().unwrap();
        let later = tempfile::tempdir().unwrap();
        let sentinel = first.path().join("ran-sentinel");
        let name = if cfg!(windows) {
            "claude.cmd"
        } else {
            "claude"
        };
        let script = format!("echo executed > \"{}\"\n", sentinel.display());
        let command = first.path().join(name);
        std::fs::write(&command, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&command, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        npm_fixture(
            later.path(),
            br#"{"name":"@anthropic-ai/claude-code","version":"2.1.302"}"#,
        );
        assert_eq!(lookup("claude", &[first.path(), later.path()]), None);
        assert_eq!(installed(command.to_str().unwrap()), None);
        assert!(!sentinel.exists());
    }

    #[test]
    fn production_lookup_reads_an_evident_native_version_directory() {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("claude/versions/2.1.302");
        std::fs::create_dir_all(&bin).unwrap();
        let name = if cfg!(windows) {
            "claude.exe"
        } else {
            "claude"
        };
        let magic = if cfg!(windows) {
            &b"MZ\0\0"[..]
        } else if cfg!(target_os = "macos") {
            &b"\xcf\xfa\xed\xfe"[..]
        } else {
            &b"\x7fELF"[..]
        };
        std::fs::write(bin.join(name), magic).unwrap();
        assert_eq!(lookup("claude", &[&bin]), Some(CliVersion::new(2, 1, 302)));
    }

    #[test]
    fn metadata_read_requires_a_bounded_regular_file() {
        let root = tempfile::tempdir().unwrap();
        let manifest = root.path().join("package.json");
        std::fs::create_dir(&manifest).unwrap();
        assert_eq!(bounded_regular_file(&manifest, 64 * 1024), None);
        let file = root.path().join("growing.json");
        std::fs::write(&file, b"12345").unwrap();
        assert_eq!(bounded_regular_file(&file, 4), None);
        assert_eq!(bounded_regular_file(&file, 5), Some(b"12345".to_vec()));
    }

    #[cfg(unix)]
    #[test]
    fn production_lookup_follows_an_npm_bin_symlink() {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        npm_fixture(
            root.path(),
            br#"{"name":"@anthropic-ai/claude-code","version":"2.1.251"}"#,
        );
        std::os::unix::fs::symlink(
            root.path()
                .join("node_modules/@anthropic-ai/claude-code/cli.js"),
            bin.join("claude"),
        )
        .unwrap();
        assert_eq!(lookup("claude", &[&bin]), Some(CliVersion::new(2, 1, 251)));
    }

    #[cfg(windows)]
    #[test]
    fn windows_resource_reader_rejects_invalid_pe_and_relative_names() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("claude.exe");
        std::fs::write(&file, b"MZ\0\0not a PE resource").unwrap();
        assert_eq!(windows_resource::version(&file), None);
        assert_eq!(windows_resource::version(Path::new("claude.exe")), None);
    }

    #[cfg(windows)]
    #[test]
    fn windows_resource_subblocks_require_in_buffer_nonoverflowing_ranges() {
        let buffer = [1u8, 2, 3, 4, 5, 6];
        let start = buffer.as_ptr() as usize;
        assert_eq!(
            windows_resource::checked_range(&buffer, start + 1, 4),
            Some(&buffer[1..5])
        );
        for (address, length) in [
            (0, 4),
            (start - 1, 4),
            (start + 5, 2),
            (start, 0),
            (start, usize::MAX),
            (usize::MAX, 4),
        ] {
            assert_eq!(
                windows_resource::checked_range(&buffer, address, length),
                None
            );
        }
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "read-only host check; set ZEST_TEST_CLAUDE_EXE and ZEST_TEST_CLAUDE_VERSION"]
    fn installed_windows_resource_metadata() {
        let file = std::path::PathBuf::from(
            std::env::var_os("ZEST_TEST_CLAUDE_EXE").expect("absolute Claude exe path"),
        );
        assert!(file.is_absolute());
        let expected = exact_version(
            &std::env::var("ZEST_TEST_CLAUDE_VERSION").expect("expected exact release"),
        )
        .expect("major.minor.patch release");
        assert_eq!(windows_resource::version(&file), Some(expected));
        assert_eq!(candidate_version(&file), Some(expected));
        assert_eq!(lookup("claude", &[file.parent().unwrap()]), Some(expected));
        assert_eq!(installed(file.to_str().unwrap()), None);
    }
}
