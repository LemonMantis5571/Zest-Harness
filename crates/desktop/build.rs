//! Embeds Zest's icon and version into the Windows executable. GPUI loads icon resource 1
//! for its windows, so this also sets the taskbar and title-bar icon.

fn main() {
    println!("cargo:rerun-if-changed=zest.rc");
    println!("cargo:rerun-if-changed=icons/icon.ico");
    println!("cargo:rerun-if-changed=../../Cargo.toml");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let version = workspace_version();
    let mut numbers: Vec<String> = version
        .split(['.', '-'])
        .take(3)
        .map(|part| part.parse::<u16>().unwrap_or(0).to_string())
        .collect();
    numbers.resize(4, "0".into());
    let macros = [
        format!("ZEST_VERSION_NUMBERS={}", numbers.join(",")),
        format!("ZEST_VERSION_TEXT=\"{version}\""),
    ];
    embed_resource::compile("zest.rc", macros)
        .manifest_optional()
        .expect("embed the Windows icon and version resource");
}

/// The release version from the repository's `[workspace.package]`.
fn workspace_version() -> String {
    let manifest = std::fs::read_to_string("../../Cargo.toml").expect("read the workspace Cargo.toml");
    let mut in_package = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[workspace.package]";
        } else if in_package
            && let Some(value) = line.strip_prefix("version").map(str::trim_start)
            && let Some(value) = value.strip_prefix('=')
        {
            return value.trim().trim_matches('"').to_string();
        }
    }
    panic!("[workspace.package] has no version");
}
