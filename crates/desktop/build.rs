use std::path::Path;
use std::process::Command;

fn main() {
    let dist = Path::new("ui/dist/index.html");
    if !dist.exists() {
        println!("cargo:warning=ui/dist missing - running npm run build --prefix ui");
        // `Command` does not apply Windows' PATHEXT lookup, so invoking the
        // extension-less `npm` binary fails even when npm is on PATH. Keep the
        // command portable for both native Windows builds and Unix CI.
        let npm = if cfg!(windows) { "npm.cmd" } else { "npm" };
        let status = Command::new(npm)
            .args(["run", "build", "--prefix", "ui"])
            .status()
            .expect(
                "failed to spawn npm - install Node.js and run: npm install --prefix crates/desktop/ui",
            );
        if !status.success() {
            panic!("npm run build --prefix ui failed; run it manually first");
        }
    }

    println!("cargo:rerun-if-changed=ui/dist/index.html");
    println!("cargo:rerun-if-changed=tauri.conf.json");
    let msvc_windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    let attributes = if msvc_windows {
        tauri_build::Attributes::new()
            .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest())
    } else {
        tauri_build::Attributes::new()
    };
    tauri_build::try_build(attributes).expect("Tauri build failed");
    if msvc_windows {
        // Embed Tauri's default Common Controls dependency in every executable,
        // including library tests. The binary-only resource is kept for icons.
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'");
    }
}
