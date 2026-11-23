//! Per-chat private pipes to system webviews. No web page has a host IPC handler.
use async_trait::async_trait;
use serde_json::Value;
use std::{io, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::Mutex,
};
use zest_core::{BrowserAction, BrowserAdapter, BrowserRequest, html::HtmlDocument};

pub const BROWSER: &str = "--zest-browser-window";
pub const ARTIFACT: &str = "--zest-artifact-window";
/// The document window drawing a diagram or math with the bundled libraries.
pub const RENDER: &str = "--zest-render-window";
const MAX_REPLY: u64 = 2 * 1024 * 1024;
pub type Reply = Result<Value, String>;

struct Session {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    _profile: tempfile::TempDir,
}

impl Session {
    fn launch(mode: &str) -> io::Result<Self> {
        let profile = tempfile::tempdir()?;
        let mut cmd = Command::new(std::env::current_exe()?);
        cmd.arg(mode).current_dir(profile.path()).env_clear();
        // The GUI needs the user's display and packaged libraries, not provider tokens.
        for name in [
            "SYSTEMROOT",
            "WINDIR",
            "LANG",
            "LC_ALL",
            "LC_CTYPE",
            "TZ",
            "DISPLAY",
            "WAYLAND_DISPLAY",
            "XDG_RUNTIME_DIR",
            "XAUTHORITY",
            "DBUS_SESSION_BUS_ADDRESS",
            "APPDIR",
            "LD_LIBRARY_PATH",
            "GDK_PIXBUF_MODULE_FILE",
            "GSETTINGS_SCHEMA_DIR",
            "XDG_DATA_DIRS",
        ] {
            if let Some(value) = std::env::var_os(name) {
                cmd.env(name, value);
            }
        }
        #[cfg(unix)]
        cmd.env("PATH", "/usr/bin:/bin");
        for name in [
            "HOME",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "TEMP",
            "TMP",
            "TMPDIR",
            "XDG_CONFIG_HOME",
            "XDG_CACHE_HOME",
            "XDG_DATA_HOME",
        ] {
            cmd.env(name, profile.path());
        }
        #[cfg(windows)]
        cmd.creation_flags(0x08000000);
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let input = child.stdin.take().expect("piped input");
        let output = BufReader::new(child.stdout.take().expect("piped output"));
        Ok(Self {
            child,
            input,
            output,
            _profile: profile,
        })
    }
    async fn send(&mut self, value: &impl serde::Serialize) -> Result<(), String> {
        let mut line = serde_json::to_vec(value).map_err(|e| e.to_string())?;
        line.push(b'\n');
        self.input
            .write_all(&line)
            .await
            .map_err(|e| format!("Web window closed: {e}"))
    }
    async fn reply(&mut self) -> Reply {
        let mut line = Vec::new();
        (&mut self.output)
            .take(MAX_REPLY + 1)
            .read_until(b'\n', &mut line)
            .await
            .map_err(|e| e.to_string())?;
        if line.is_empty() {
            return Err("Web window closed".into());
        }
        if line.len() as u64 > MAX_REPLY {
            return Err("Web window response exceeded its size limit".into());
        }
        serde_json::from_slice::<Reply>(&line)
            .map_err(|_| "Invalid web window response".to_string())?
    }
}

#[derive(Default)]
pub struct Browser(Mutex<Option<Session>>);

#[async_trait]
impl BrowserAdapter for Browser {
    async fn execute(&self, request: BrowserRequest) -> Reply {
        let mut slot = self.0.lock().await;
        if let Some(existing) = slot.as_mut() {
            if existing
                .child
                .try_wait()
                .map_err(|e| e.to_string())?
                .is_some()
            {
                *slot = None;
            }
        }
        let mut session = match slot.take() {
            Some(existing) => existing,
            _ if request.action == BrowserAction::Open => {
                Session::launch(BROWSER).map_err(|e| e.to_string())?
            }
            _ => return Err("No browser page is open. Open a URL first.".into()),
        };
        // Cancellation drops the in-flight window. A later call cannot consume its stale reply.
        let result = tokio::time::timeout(Duration::from_millis(request.timeout_ms()), async {
            loop {
                session.send(&request).await?;
                let value = session.reply().await?;
                if request.action != BrowserAction::Wait
                    || value[if request.locator.is_some() {
                        "found"
                    } else {
                        "ready"
                    }] == true
                {
                    return Ok(value);
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .map_err(|_| "Browser action timed out".to_string())?;
        // Only retain a synchronized stream after a complete exchange.
        if result.is_ok() {
            *slot = Some(session);
        }
        result
    }
}

pub async fn show_artifact(document: &HtmlDocument) -> Result<(), String> {
    show(ARTIFACT, document).await
}

pub async fn show_visual(visual: &crate::visual::Visual) -> Result<(), String> {
    show(RENDER, visual).await
}

async fn show(mode: &str, request: &impl serde::Serialize) -> Result<(), String> {
    let mut session = Session::launch(mode).map_err(|e| e.to_string())?;
    session.send(request).await?;
    tokio::time::timeout(Duration::from_secs(20), session.reply())
        .await
        .map_err(|_| "The interactive document took too long to open".to_owned())??;
    // The child closes on parent EOF or when the viewer window closes.
    session.child.wait().await.map_err(|e| e.to_string())?;
    Ok(())
}
