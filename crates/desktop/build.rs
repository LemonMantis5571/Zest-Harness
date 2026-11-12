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
    let windows_target = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    let attributes = if windows_target {
        tauri_build::Attributes::new()
            .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest())
    } else {
        tauri_build::Attributes::new()
    };
    tauri_build::try_build(attributes).expect("Tauri build failed");
    if windows_target {
        // Embed Tauri's default Common Controls dependency in every executable,
        // including library tests. The binary-only resource is kept for icons.
        println!("cargo:rerun-if-changed=windows-common-controls.rc");
        println!("cargo:rerun-if-changed=windows-common-controls.manifest");
        embed_resource::compile_for_everything("windows-common-controls.rc", embed_resource::NONE)
            .manifest_required()
            .expect("Common Controls manifest compilation failed");
    }
}
