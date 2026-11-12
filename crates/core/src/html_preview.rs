//! Disposable installed-browser previews. Confinement applies to the document,
//! not to browser background traffic or a compromised browser implementation.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD, Engine};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::{protocol::WebSocketConfig, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::html::{HtmlDocument, HTML_CSP};
use crate::process_job::ProcessJob;

const PREVIEW_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_PNG_BYTES: usize = 2 * 1024 * 1024;
const MAX_CDP_BYTES: usize = 4 * 1024 * 1024;
const MAX_DIAGNOSTICS: usize = 64;
const MAX_DIAGNOSTIC_CHARS: usize = 1024;
const WIDTH: u32 = 1024;
const HEIGHT: u32 = 768;

#[derive(Debug)]
pub struct PreviewReport {
    pub png_base64: String,
    pub diagnostics: Vec<String>,
}

pub struct HtmlPreviewer {
    browser: Option<PathBuf>,
}

impl HtmlPreviewer {
    /// Finds an existing browser; never installs one or uses a user's profile.
    pub fn installed() -> Self {
        Self {
            browser: installed_browser(),
        }
    }

    pub async fn preview(&self, document: &HtmlDocument) -> Result<PreviewReport, String> {
        let browser = self.browser.as_deref().ok_or_else(|| {
            "HTML preview engine unavailable: install Chromium, Chrome, or Microsoft Edge"
                .to_string()
        })?;
        tokio::time::timeout(PREVIEW_TIMEOUT, render(browser, document))
            .await
            .map_err(|_| "HTML preview timed out after 20 seconds; browser stopped".to_string())?
    }
}

fn installed_browser() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    #[cfg(windows)]
    {
        for root in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
            if let Some(root) = std::env::var_os(root) {
                let root = PathBuf::from(root);
                candidates.push(root.join("Microsoft/Edge/Application/msedge.exe"));
                candidates.push(root.join("Google/Chrome/Application/chrome.exe"));
                candidates.push(root.join("Chromium/Application/chrome.exe"));
            }
        }
    }
    #[cfg(target_os = "macos")]
    candidates.extend([
        PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
        PathBuf::from("/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"),
        PathBuf::from("/Applications/Chromium.app/Contents/MacOS/Chromium"),
    ]);
    if let Some(path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path) {
            for name in [
                "chromium",
                "chromium-browser",
                "google-chrome",
                "microsoft-edge",
            ] {
                #[cfg(windows)]
                let name = format!("{name}.exe");
                candidates.push(directory.join(name));
            }
        }
    }
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .and_then(|path| path.canonicalize().ok())
}

fn browser_environment(command: &mut Command, profile: &Path) {
    command.env_clear();
    for name in ["SYSTEMROOT", "WINDIR", "LANG", "LC_ALL", "LC_CTYPE", "TZ"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    // Distribution-provided Chrome launchers can invoke system utilities.
    // Never inherit the host's PATH, which may contain credential wrappers.
    #[cfg(unix)]
    command.env("PATH", "/usr/bin:/bin");
    // Chromium may consult these independently of --user-data-dir.
    for name in [
        "HOME",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "XDG_CONFIG_HOME",
        "XDG_CACHE_HOME",
        "XDG_DATA_HOME",
        "TMP",
        "TEMP",
        "TMPDIR",
    ] {
        command.env(name, profile);
    }
    command.current_dir(profile);
}

/// A synchronous child is intentional: Drop must kill and reap it even when
/// the async runtime is shutting down or the preview future is cancelled.
struct BrowserProcess {
    child: Child,
    job: Option<ProcessJob>,
    profile: Option<TempDir>,
    stopped: bool,
}

impl BrowserProcess {
    fn start(executable: &Path) -> Result<Self, String> {
        let profile = tempfile::Builder::new()
            .prefix("zest-html-preview-")
            .tempdir()
            .map_err(|e| format!("HTML preview cannot create disposable profile: {e}"))?;
        let mut command = Command::new(executable);
        browser_environment(&mut command, profile.path());
        command
            .args([
                "--headless=new",
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-extensions",
                "--disable-background-networking",
                "--disable-component-update",
                "--disable-sync",
                "--disable-default-apps",
                "--disable-breakpad",
                "--disable-client-side-phishing-detection",
                "--disable-domain-reliability",
                "--metrics-recording-only",
                "--password-store=basic",
                "--use-mock-keychain",
                "--remote-debugging-address=127.0.0.1",
                "--remote-debugging-port=0",
                "--host-resolver-rules=MAP * ~NOTFOUND",
                "--proxy-server=http://127.0.0.1:9",
                "--proxy-bypass-list=<-loopback>",
                "--force-device-scale-factor=1",
                "--window-size=1024,768",
                "--mute-audio",
            ])
            .arg(format!("--user-data-dir={}", profile.path().display()))
            .arg("about:blank")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(crate::process_job::windows_creation_flags());
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let job = ProcessJob::new();
        #[cfg(windows)]
        if job.is_none() {
            return Err(
                "HTML preview confinement failed: cannot create browser process job".into(),
            );
        }
        let child = command
            .spawn()
            .map_err(|e| format!("HTML preview engine unavailable: cannot launch browser: {e}"))?;
        let process = Self {
            child,
            job,
            profile: Some(profile),
            stopped: false,
        };
        #[cfg(windows)]
        {
            if !process
                .job
                .as_ref()
                .is_some_and(|job| job.assign(process.child.id()))
            {
                return Err(
                    "HTML preview confinement failed: cannot own browser process tree".into(),
                );
            }
            crate::process_job::resume_process(process.child.id());
        }
        Ok(process)
    }

    async fn endpoint(&mut self) -> Result<String, String> {
        let path = self
            .profile
            .as_ref()
            .unwrap()
            .path()
            .join("DevToolsActivePort");
        loop {
            if let Some(status) = self.child.try_wait().map_err(|e| e.to_string())? {
                return Err(format!(
                    "HTML preview engine unavailable: browser exited ({status})"
                ));
            }
            if let Ok(contents) = tokio::fs::read_to_string(&path).await {
                if let Ok(endpoint) = parse_endpoint(&contents) {
                    return Ok(endpoint);
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    fn cleanup(&mut self) -> Result<(), String> {
        if !self.stopped {
            if let Some(job) = self.job.take() {
                job.terminate();
                drop(job);
            }
            #[cfg(not(windows))]
            crate::process_job::terminate_process_tree(self.child.id());
            let _ = self.child.kill();
            self.child
                .wait()
                .map_err(|e| format!("HTML preview cannot reap browser: {e}"))?;
            self.stopped = true;
        }
        if let Some(profile) = self.profile.as_ref() {
            // Job termination can precede the final child releasing its file handles.
            let mut removed = false;
            let mut last_error = None;
            for _ in 0..25 {
                match remove_profile(profile.path()) {
                    Ok(()) => {
                        removed = true;
                        break;
                    }
                    Err(e)
                        if e.kind() == std::io::ErrorKind::NotFound
                            && std::fs::symlink_metadata(profile.path()).is_err_and(|error| {
                                error.kind() == std::io::ErrorKind::NotFound
                            }) =>
                    {
                        removed = true;
                        break;
                    }
                    Err(error) => {
                        last_error = Some(error);
                        std::thread::sleep(Duration::from_millis(20));
                    }
                }
            }
            if !removed {
                return Err(format!(
                    "HTML preview could not remove its disposable profile: {}",
                    last_error.unwrap()
                ));
            }
        }
        self.profile.take();
        Ok(())
    }
}

impl Drop for BrowserProcess {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

/// Edge creates a WinINet junction which denies directory enumeration. Unlink
/// reparse points before walking directories, and never follow their targets.
fn remove_profile(path: &Path) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    let kind = metadata.file_type();
    if kind.is_symlink() {
        #[cfg(windows)]
        {
            use std::os::windows::fs::FileTypeExt;
            if kind.is_symlink_dir() {
                return std::fs::remove_dir(path);
            }
        }
        return std::fs::remove_file(path);
    }
    if kind.is_dir() {
        for entry in std::fs::read_dir(path)? {
            remove_profile(&entry?.path())?;
        }
        return std::fs::remove_dir(path);
    }
    #[cfg(windows)]
    if metadata.permissions().readonly() {
        let mut permissions = metadata.permissions();
        // Only Windows' readonly attribute changes; this branch is absent on Unix.
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        std::fs::set_permissions(path, permissions)?;
    }
    std::fs::remove_file(path)
}

fn parse_endpoint(contents: &str) -> Result<String, String> {
    let mut lines = contents.lines();
    let port = lines
        .next()
        .and_then(|line| line.parse::<u16>().ok())
        .filter(|port| *port != 0)
        .ok_or("invalid browser debugging port")?;
    let path = lines.next().ok_or("missing browser debugging path")?;
    if !path.starts_with("/devtools/browser/")
        || !path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/-".contains(&b))
    {
        return Err("invalid browser debugging path".into());
    }
    Ok(format!("ws://127.0.0.1:{port}{path}"))
}

struct Routes {
    wrapper_url: String,
    document_url: String,
    wrapper_body: String,
    document_body: String,
    wrapper_csp: String,
    wrapper_served: bool,
    document_served: bool,
    document_frame: Option<String>,
}

impl Routes {
    fn new(document: &HtmlDocument) -> Result<Self, String> {
        let mut token = [0_u8; 16];
        getrandom::fill(&mut token)
            .map_err(|e| format!("HTML preview cannot create routes: {e}"))?;
        let token: String = token.iter().map(|b| format!("{b:02x}")).collect();
        let root = format!("https://zest-html.invalid/{token}");
        let document_url = format!("{root}/document");
        Ok(Self {
            wrapper_url: format!("{root}/wrapper"),
            wrapper_body: STANDARD.encode(format!(
                "<!doctype html><meta charset=utf-8><style>html,body{{margin:0;width:100%;height:100%;overflow:hidden}}iframe{{display:block;border:0;width:100%;height:100%}}</style><iframe sandbox=\"allow-scripts\" referrerpolicy=\"no-referrer\" src=\"{document_url}\"></iframe>"
            )),
            document_body: STANDARD.encode(document.isolated_html()),
            wrapper_csp: format!(
                "default-src 'none'; style-src 'unsafe-inline'; frame-src {document_url}; connect-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'"
            ),
            document_url,
            wrapper_served: false,
            document_served: false,
            document_frame: None,
        })
    }

    /// No request is ever continued. Only these two one-time document loads
    /// receive locally synthesized responses; the browser has no HTML server.
    fn response(&mut self, params: &Value, main_frame: &str) -> Option<Value> {
        if params["request"]["method"] != "GET" || params["resourceType"] != "Document" {
            return None;
        }
        let url = params["request"]["url"].as_str()?;
        let frame = params["frameId"].as_str()?;
        let (body, csp) = if url == self.wrapper_url && frame == main_frame && !self.wrapper_served
        {
            self.wrapper_served = true;
            (&self.wrapper_body, self.wrapper_csp.as_str())
        } else if url == self.document_url && frame != main_frame && !self.document_served {
            self.document_served = true;
            self.document_frame = Some(frame.to_owned());
            (&self.document_body, HTML_CSP)
        } else {
            return None;
        };
        Some(json!({
            "requestId": params["requestId"], "responseCode": 200, "body": body,
            "responseHeaders": [
                {"name":"Content-Type", "value":"text/html; charset=utf-8"},
                {"name":"Content-Security-Policy", "value":csp},
                {"name":"Cache-Control", "value":"no-store"},
                {"name":"Referrer-Policy", "value":"no-referrer"},
                {"name":"X-Content-Type-Options", "value":"nosniff"},
                {"name":"Permissions-Policy", "value":"camera=(), microphone=(), geolocation=(), usb=(), serial=(), bluetooth=(), payment=()"},
            ]
        }))
    }
}

enum Pending {
    Checked(&'static str),
    Setup(String, &'static str),
}

struct Cdp {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    next_id: u64,
    pending: HashMap<u64, Pending>,
    setup_remaining: HashMap<String, usize>,
    routes: Routes,
    main_session: String,
    main_frame: String,
    frame_parents: HashMap<String, String>,
    child_sessions: HashMap<String, String>,
    loaded: bool,
    diagnostics: Vec<String>,
}

fn session_setup() -> Vec<(&'static str, Value)> {
    vec![
        ("Page.enable", json!({})),
        ("Runtime.enable", json!({})),
        // Covers new child contexts too, before any generated srcdoc script.
        (
            "Page.addScriptToEvaluateOnNewDocument",
            json!({"source":"for(const name of ['RTCPeerConnection','webkitRTCPeerConnection']){Object.defineProperty(globalThis,name,{value:undefined,writable:false,configurable:false});}"}),
        ),
        ("Log.enable", json!({})),
        ("Network.enable", json!({})),
        ("Network.setBypassServiceWorker", json!({"bypass":true})),
        ("Network.setCacheDisabled", json!({"cacheDisabled":true})),
        (
            "Fetch.enable",
            json!({"patterns":[{"urlPattern":"*","requestStage":"Request"}],"handleAuthRequests":true}),
        ),
        (
            "Target.setAutoAttach",
            json!({"autoAttach":true,"waitForDebuggerOnStart":true,"flatten":true}),
        ),
    ]
}

impl Cdp {
    async fn connect(endpoint: &str, document: &HtmlDocument) -> Result<Self, String> {
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_CDP_BYTES))
            .max_frame_size(Some(MAX_CDP_BYTES));
        let (socket, _) =
            tokio_tungstenite::connect_async_with_config(endpoint, Some(config), true)
                .await
                .map_err(|e| format!("HTML preview cannot connect to local browser: {e}"))?;
        Ok(Self {
            socket,
            next_id: 0,
            pending: HashMap::new(),
            setup_remaining: HashMap::new(),
            routes: Routes::new(document)?,
            main_session: String::new(),
            main_frame: String::new(),
            frame_parents: HashMap::new(),
            child_sessions: HashMap::new(),
            loaded: false,
            diagnostics: Vec::new(),
        })
    }

    async fn send(
        &mut self,
        method: &'static str,
        params: Value,
        session: &str,
    ) -> Result<u64, String> {
        if self.pending.len() >= 256 {
            return Err(
                "HTML preview confinement failed: too many pending browser requests".into(),
            );
        }
        self.next_id += 1;
        let mut message = json!({"id":self.next_id,"method":method,"params":params});
        if !session.is_empty() {
            message["sessionId"] = session.into();
        }
        self.socket
            .send(Message::Text(message.to_string().into()))
            .await
            .map_err(|e| format!("HTML preview debugging connection failed: {e}"))?;
        Ok(self.next_id)
    }

    async fn checked(
        &mut self,
        method: &'static str,
        params: Value,
        session: &str,
    ) -> Result<(), String> {
        let id = self.send(method, params, session).await?;
        self.pending.insert(id, Pending::Checked(method));
        Ok(())
    }

    async fn command(
        &mut self,
        method: &'static str,
        params: Value,
        session: &str,
    ) -> Result<Value, String> {
        let id = self.send(method, params, session).await?;
        loop {
            if let Some((response_id, response)) = self.step().await? {
                if response_id == id {
                    return protocol_result(method, response);
                }
                return Err("HTML preview received an unexpected protocol response".into());
            }
        }
    }

    async fn step(&mut self) -> Result<Option<(u64, Value)>, String> {
        let message = self
            .socket
            .next()
            .await
            .ok_or("HTML preview browser disconnected")?
            .map_err(|e| format!("HTML preview debugging connection failed: {e}"))?;
        self.consume(message).await
    }

    async fn consume(&mut self, message: Message) -> Result<Option<(u64, Value)>, String> {
        let text = match message {
            Message::Text(text) => text,
            Message::Ping(_) | Message::Pong(_) => return Ok(None),
            _ => return Err("HTML preview browser sent an unexpected debugging message".into()),
        };
        let value: Value = serde_json::from_str(&text)
            .map_err(|e| format!("HTML preview received invalid debugging data: {e}"))?;
        if let Some(id) = value["id"].as_u64() {
            match self.pending.remove(&id) {
                Some(Pending::Checked(method)) => {
                    protocol_result(method, value)?;
                }
                Some(Pending::Setup(session, method)) => {
                    protocol_result(method, value)?;
                    let remaining = self
                        .setup_remaining
                        .get_mut(&session)
                        .ok_or("HTML preview lost child confinement state")?;
                    *remaining -= 1;
                    if *remaining == 0 {
                        self.setup_remaining.remove(&session);
                        self.checked("Runtime.runIfWaitingForDebugger", json!({}), &session)
                            .await?;
                    }
                }
                None => return Ok(Some((id, value))),
            }
        } else {
            self.event(value).await?;
        }
        Ok(None)
    }

    fn diagnostic(&mut self, text: String) {
        if self.diagnostics.len() < MAX_DIAGNOSTICS {
            self.diagnostics
                .push(text.chars().take(MAX_DIAGNOSTIC_CHARS).collect());
        }
    }

    async fn event(&mut self, value: Value) -> Result<(), String> {
        let session = value["sessionId"].as_str().unwrap_or("");
        let params = &value["params"];
        match value["method"].as_str().unwrap_or("") {
            "Fetch.requestPaused" => {
                if params["request"]["url"] == self.routes.document_url
                    && !params["frameId"].as_str().is_some_and(|frame| {
                        self.frame_parents.get(frame) == Some(&self.main_frame)
                    })
                {
                    return Err(
                        "HTML preview confinement failed: document has an unexpected parent".into(),
                    );
                }
                if let Some(response) = self.routes.response(params, &self.main_frame) {
                    self.checked("Fetch.fulfillRequest", response, session)
                        .await?;
                } else {
                    self.diagnostic(format!(
                        "Blocked request: {}",
                        params["request"]["url"].as_str().unwrap_or("unknown URL")
                    ));
                    self.checked(
                        "Fetch.failRequest",
                        json!({"requestId":params["requestId"],"errorReason":"BlockedByClient"}),
                        session,
                    )
                    .await?;
                }
            }
            "Fetch.authRequired" => {
                self.checked("Fetch.continueWithAuth", json!({"requestId":params["requestId"],"authChallengeResponse":{"response":"CancelAuth"}}), session).await?;
            }
            "Target.attachedToTarget" => {
                let child_session = params["sessionId"]
                    .as_str()
                    .ok_or("missing attached session")?;
                // The manual initial attachment has no executing document yet.
                if !params["waitingForDebugger"].as_bool().unwrap_or(false) {
                    if self.main_session.is_empty() || child_session == self.main_session {
                        return Ok(());
                    }
                    return Err(
                        "HTML preview confinement failed: child target was not paused".into(),
                    );
                }
                let frame = params["targetInfo"]["targetId"]
                    .as_str()
                    .ok_or("HTML preview missing attached frame ID")?;
                let parent = params["targetInfo"]["parentFrameId"].as_str();
                if params["targetInfo"]["type"] != "iframe"
                    || session != self.main_session
                    || self.frame_parents.get(frame) != Some(&self.main_frame)
                    || parent.is_some_and(|parent| parent != self.main_frame)
                    || !self.child_sessions.is_empty()
                {
                    return Err("HTML preview confinement failed: unexpected browser target".into());
                }
                self.child_sessions
                    .insert(frame.into(), child_session.into());
                let setup = session_setup();
                self.setup_remaining
                    .insert(child_session.into(), setup.len());
                for (method, arguments) in setup {
                    let id = self.send(method, arguments, child_session).await?;
                    self.pending
                        .insert(id, Pending::Setup(child_session.into(), method));
                }
            }
            "Page.loadEventFired" if session == self.main_session => self.loaded = true,
            "Page.frameAttached" => {
                let frame = params["frameId"]
                    .as_str()
                    .ok_or("missing attached frame ID")?;
                let parent = params["parentFrameId"]
                    .as_str()
                    .ok_or("missing parent frame ID")?;
                if self.frame_parents.len() >= 16 {
                    return Err("HTML preview confinement failed: too many child frames".into());
                }
                self.frame_parents.insert(frame.into(), parent.into());
            }
            "Page.frameDetached" if params["reason"] == "remove" => {
                if let Some(frame) = params["frameId"].as_str() {
                    self.frame_parents.remove(frame);
                }
            }
            "Target.detachedFromTarget" => {
                if let Some(detached) = params["sessionId"].as_str() {
                    self.child_sessions.retain(|_, session| session != detached);
                }
            }
            "Page.javascriptDialogOpening" => {
                self.diagnostic("Blocked JavaScript dialog".into());
                self.checked(
                    "Page.handleJavaScriptDialog",
                    json!({"accept":false}),
                    session,
                )
                .await?;
            }
            "Runtime.consoleAPICalled" => {
                let arguments = params["args"]
                    .as_array()
                    .map(|args| {
                        args.iter()
                            .take(8)
                            .map(|arg| {
                                if let Some(value) = arg.get("value") {
                                    value
                                        .as_str()
                                        .map(str::to_owned)
                                        .unwrap_or_else(|| value.to_string())
                                } else {
                                    arg["description"].as_str().unwrap_or("[object]").to_owned()
                                }
                            })
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .unwrap_or_default();
                self.diagnostic(format!(
                    "Console {}: {arguments}",
                    params["type"].as_str().unwrap_or("log")
                ));
            }
            "Runtime.exceptionThrown" => {
                let details = &params["exceptionDetails"];
                let description = details["exception"]["description"]
                    .as_str()
                    .or_else(|| details["text"].as_str())
                    .unwrap_or("JavaScript exception");
                self.diagnostic(format!("JavaScript error: {description}"));
            }
            "Log.entryAdded" => {
                let entry = &params["entry"];
                self.diagnostic(format!(
                    "Browser {}: {}",
                    entry["level"].as_str().unwrap_or("log"),
                    entry["text"].as_str().unwrap_or("")
                ));
            }
            "Network.loadingFailed" => {
                self.diagnostic(format!(
                    "Request failed: {}",
                    params["errorText"].as_str().unwrap_or("blocked")
                ));
            }
            "Inspector.targetCrashed" | "Target.targetCrashed" => {
                return Err("HTML preview browser target crashed".into());
            }
            _ => {}
        }
        Ok(())
    }

    async fn verify_frames(&mut self) -> Result<(), String> {
        let main_session = self.main_session.clone();
        let main = self
            .command("Page.getFrameTree", json!({}), &main_session)
            .await?;
        let document_frame = self
            .routes
            .document_frame
            .clone()
            .ok_or("HTML preview confinement failed: document was not served")?;
        let child_session = self.child_sessions.get(&document_frame).cloned();
        let document = if let Some(session) = &child_session {
            Some(
                self.command("Page.getFrameTree", json!({}), session)
                    .await?,
            )
        } else {
            None
        };
        validate_frame_trees(
            &main,
            document.as_ref(),
            &self.main_frame,
            &document_frame,
            &self.routes.wrapper_url,
            &self.routes.document_url,
        )?;
        if self.frame_parents.get(&document_frame) != Some(&self.main_frame)
            || self.frame_parents.len() != 1
            || self.child_sessions.len() != usize::from(child_session.is_some())
        {
            return Err("HTML preview confinement failed: document frame ancestry changed".into());
        }
        Ok(())
    }
}

fn validate_frame_trees(
    main: &Value,
    document: Option<&Value>,
    main_frame: &str,
    document_frame: &str,
    wrapper_url: &str,
    document_url: &str,
) -> Result<(), String> {
    let main = &main["frameTree"];
    let children = main["childFrames"].as_array();
    let child = if let Some(document) = document {
        // An out-of-process child disappears from the parent's Page tree.
        // Its own Page tree must still prove the exact ID, parent and URL.
        if children.is_some_and(|children| !children.is_empty()) {
            return Err("HTML preview confinement failed: unexpected in-process child".into());
        }
        &document["frameTree"]
    } else {
        let children = children
            .filter(|children| children.len() == 1)
            .ok_or("HTML preview confinement failed: missing document frame")?;
        &children[0]
    };
    if child
        .get("childFrames")
        .is_some_and(|children| !children.as_array().is_some_and(Vec::is_empty))
    {
        return Err("HTML preview confinement failed: document contains nested frames".into());
    }
    let child = &child["frame"];
    if main["frame"]["id"] != main_frame
        || main["frame"]["url"] != wrapper_url
        || child["id"] != document_frame
        || child["parentId"] != main_frame
        || child["url"] != document_url
    {
        return Err("HTML preview confinement failed: document navigated outside its frame".into());
    }
    Ok(())
}

fn protocol_result(method: &str, response: Value) -> Result<Value, String> {
    if let Some(error) = response.get("error") {
        return Err(format!(
            "HTML preview confinement/engine command {method} failed: {error}"
        ));
    }
    response
        .get("result")
        .cloned()
        .ok_or_else(|| "HTML preview missing protocol result".into())
}

async fn render(executable: &Path, document: &HtmlDocument) -> Result<PreviewReport, String> {
    let mut browser = BrowserProcess::start(executable)?;
    let endpoint = browser.endpoint().await?;
    let mut cdp = Cdp::connect(&endpoint, document).await?;
    cdp.command(
        "Browser.setDownloadBehavior",
        json!({"behavior":"deny"}),
        "",
    )
    .await?;
    let targets = cdp.command("Target.getTargets", json!({}), "").await?;
    let target = targets["targetInfos"]
        .as_array()
        .and_then(|targets| {
            targets
                .iter()
                .find(|target| target["type"] == "page" && target["url"] == "about:blank")
        })
        .ok_or("HTML preview engine did not create a blank page")?;
    let attached = cdp
        .command(
            "Target.attachToTarget",
            json!({"targetId":target["targetId"],"flatten":true}),
            "",
        )
        .await?;
    let session = attached["sessionId"]
        .as_str()
        .ok_or("HTML preview missing page session")?
        .to_owned();
    cdp.main_session = session.clone();
    for (method, params) in session_setup() {
        cdp.command(method, params, &session).await?;
    }
    cdp.command(
        "Emulation.setDeviceMetricsOverride",
        json!({"width":WIDTH,"height":HEIGHT,"deviceScaleFactor":1,"mobile":false}),
        &session,
    )
    .await?;
    let tree = cdp
        .command("Page.getFrameTree", json!({}), &session)
        .await?;
    cdp.main_frame = tree["frameTree"]["frame"]["id"]
        .as_str()
        .ok_or("HTML preview missing main frame")?
        .into();
    let navigation = cdp
        .command(
            "Page.navigate",
            json!({"url":cdp.routes.wrapper_url}),
            &session,
        )
        .await?;
    if let Some(error) = navigation.get("errorText") {
        return Err(format!("HTML preview document navigation failed: {error}"));
    }
    while !cdp.loaded || !cdp.routes.document_served || !cdp.pending.is_empty() {
        if cdp.step().await?.is_some() {
            return Err("HTML preview received an unexpected protocol response".into());
        }
    }
    // Allow the initial layout and short deferred scripts to produce diagnostics.
    let settle = tokio::time::sleep(Duration::from_millis(250));
    tokio::pin!(settle);
    loop {
        let message = tokio::select! {
            biased;
            _ = &mut settle => break,
            message = cdp.socket.next() => message,
        };
        let message = message
            .ok_or("HTML preview browser disconnected")?
            .map_err(|e| format!("HTML preview debugging connection failed: {e}"))?;
        cdp.consume(message).await?;
    }
    cdp.verify_frames().await?;
    let capture = cdp
        .command(
            "Page.captureScreenshot",
            json!({
                "format":"png","fromSurface":true,"captureBeyondViewport":false,
                "clip":{"x":0,"y":0,"width":WIDTH,"height":HEIGHT,"scale":1}
            }),
            &session,
        )
        .await?;
    let png_base64 = capture["data"]
        .as_str()
        .ok_or("HTML preview browser returned no PNG")?
        .to_owned();
    validate_png(&png_base64)?;
    // A capture response does not acknowledge other outstanding CDP commands.
    // Check every confinement operation before returning a successful report.
    while !cdp.pending.is_empty() {
        if cdp.step().await?.is_some() {
            return Err("HTML preview received an unexpected protocol response".into());
        }
    }
    cdp.verify_frames().await?;
    let diagnostics = std::mem::take(&mut cdp.diagnostics);
    drop(cdp);
    browser.cleanup()?;
    Ok(PreviewReport {
        png_base64,
        diagnostics,
    })
}

fn validate_png(encoded: &str) -> Result<(), String> {
    if encoded.len() > MAX_PNG_BYTES.div_ceil(3) * 4 {
        return Err("HTML preview PNG exceeds 2 MiB".into());
    }
    let png = STANDARD
        .decode(encoded)
        .map_err(|_| "HTML preview browser returned invalid PNG encoding")?;
    if png.len() > MAX_PNG_BYTES {
        return Err("HTML preview PNG exceeds 2 MiB".into());
    }
    if png.len() < 45
        || &png[..8] != b"\x89PNG\r\n\x1a\n"
        || &png[8..16] != b"\0\0\0\rIHDR"
        || u32::from_be_bytes(png[16..20].try_into().unwrap()) != WIDTH
        || u32::from_be_bytes(png[20..24].try_into().unwrap()) != HEIGHT
    {
        return Err("HTML preview browser returned an invalid 1024x768 PNG".into());
    }
    let mut offset = 8;
    let mut image_data = false;
    while offset + 12 <= png.len() {
        let length = u32::from_be_bytes(png[offset..offset + 4].try_into().unwrap()) as usize;
        let end = offset
            .checked_add(length)
            .and_then(|end| end.checked_add(12))
            .filter(|end| *end <= png.len())
            .ok_or("HTML preview PNG is truncated")?;
        let kind = &png[offset + 4..offset + 8];
        image_data |= kind == b"IDAT" && length > 0;
        if kind == b"IEND" {
            return if image_data && length == 0 && end == png.len() {
                Ok(())
            } else {
                Err("HTML preview PNG has invalid image data".into())
            };
        }
        offset = end;
    }
    Err("HTML preview PNG has no end chunk".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(source: &str) -> HtmlDocument {
        HtmlDocument::parse("Preview test".into(), source.into()).unwrap()
    }

    #[test]
    fn debugging_endpoint_cannot_redirect_to_a_remote_host() {
        assert_eq!(
            parse_endpoint("4567\n/devtools/browser/abc-123\n").unwrap(),
            "ws://127.0.0.1:4567/devtools/browser/abc-123"
        );
        for input in [
            "0\n/devtools/browser/a",
            "99999\n/devtools/browser/a",
            "80\n//remote/a",
            "80\n/devtools/browser/a?redirect=x",
            "80\n/devtools/browser/a\rHost: x",
        ] {
            assert!(parse_endpoint(input).is_err(), "{input:?}");
        }
    }

    #[test]
    fn browser_environment_discards_credentials_and_profile_paths() {
        let temporary = tempfile::tempdir().unwrap();
        let mut command = Command::new("browser");
        command
            .env("OPENAI_API_KEY", "test-secret")
            .env("HTTPS_PROXY", "test-proxy")
            .env("USERPROFILE", "private-profile")
            .env("ARBITRARY_SECRET", "test-secret");
        browser_environment(&mut command, temporary.path());
        let environment: HashMap<_, _> = command
            .get_envs()
            .filter_map(|(name, value)| {
                value.map(|value| (name.to_string_lossy().to_string(), value.to_owned()))
            })
            .collect();
        for name in ["OPENAI_API_KEY", "HTTPS_PROXY", "ARBITRARY_SECRET"] {
            assert!(!environment.contains_key(name));
        }
        #[cfg(windows)]
        assert!(!environment.contains_key("PATH"));
        #[cfg(unix)]
        assert_eq!(environment["PATH"], "/usr/bin:/bin");
        assert_eq!(environment["USERPROFILE"], temporary.path().as_os_str());
        assert_eq!(environment["HOME"], temporary.path().as_os_str());
    }

    #[test]
    fn requests_require_exact_one_time_document_routes_and_response_policy() {
        let mut routes = Routes::new(&document("<h1>hello</h1>")).unwrap();
        let mut request = json!({"requestId":"1","request":{"url":routes.document_url,"method":"GET"},"resourceType":"Document","frameId":"main"});
        assert!(routes.response(&request, "main").is_none());
        request["frameId"] = "child".into();
        request["request"]["url"] = format!("{}?extra=1", routes.document_url).into();
        assert!(routes.response(&request, "main").is_none());
        request["request"]["url"] = routes.document_url.clone().into();
        request["request"]["method"] = "POST".into();
        assert!(routes.response(&request, "main").is_none());
        request["request"]["method"] = "GET".into();
        request["resourceType"] = "Script".into();
        assert!(routes.response(&request, "main").is_none());
        request["resourceType"] = "Document".into();
        let response = routes.response(&request, "main").unwrap();
        assert!(response["responseHeaders"]
            .as_array()
            .unwrap()
            .iter()
            .any(
                |header| header["name"] == "Content-Security-Policy" && header["value"] == HTML_CSP
            ));
        assert!(routes.response(&request, "main").is_none());
        for url in [
            "file:///etc/passwd",
            "http://127.0.0.1:1234/",
            "https://example.com/",
            "data:text/html,hi",
            "blob:https://zest-html.invalid/x",
        ] {
            request["request"]["url"] = url.into();
            assert!(routes.response(&request, "main").is_none());
        }
        request["request"]["url"] = routes.wrapper_url.clone().into();
        assert!(routes.response(&request, "main").is_none());
        request["frameId"] = "main".into();
        assert!(routes.response(&request, "main").is_some());
        assert!(routes.response(&request, "main").is_none());
    }

    #[test]
    fn png_validation_rejects_fake_oversized_and_wrong_dimension_images() {
        assert!(validate_png("not base64").is_err());
        assert!(validate_png(&STANDARD.encode(b"fake screenshot")).is_err());
        assert!(validate_png(&STANDARD.encode(vec![0; MAX_PNG_BYTES + 1])).is_err());
        let tiny_png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=";
        assert!(validate_png(tiny_png).is_err());
    }

    #[test]
    fn frame_validation_checks_the_owning_session_and_exact_ancestry_and_urls() {
        let main = json!({"frameTree":{"frame":{"id":"main","url":"https://zest-html.invalid/token/wrapper"}}});
        let child = json!({"frameTree":{"frame":{"id":"child","parentId":"main","url":"https://zest-html.invalid/token/document"}}});
        let verify = |main: &Value, child: Option<&Value>| {
            validate_frame_trees(
                main,
                child,
                "main",
                "child",
                "https://zest-html.invalid/token/wrapper",
                "https://zest-html.invalid/token/document",
            )
        };
        assert!(verify(&main, Some(&child)).is_ok());
        assert!(verify(&main, None).is_err());
        let mut in_process = main.clone();
        in_process["frameTree"]["childFrames"] = json!([{"frame":child["frameTree"]["frame"]}]);
        assert!(verify(&in_process, None).is_ok());
        assert!(verify(&in_process, Some(&child)).is_err());
        for url in ["about:blank", "about:srcdoc", "https://example.com/"] {
            let grandchild = json!({"frame":{"id":"grandchild","parentId":"child","url":url}});
            let mut nested_child = child.clone();
            nested_child["frameTree"]["childFrames"] = json!([grandchild.clone()]);
            assert!(verify(&main, Some(&nested_child)).is_err());
            let mut nested_in_process = in_process.clone();
            nested_in_process["frameTree"]["childFrames"][0]["childFrames"] = json!([grandchild]);
            assert!(verify(&nested_in_process, None).is_err());
        }
        let mut empty_child = child.clone();
        empty_child["frameTree"]["childFrames"] = json!([]);
        assert!(verify(&main, Some(&empty_child)).is_ok());
        let mut empty_in_process = in_process.clone();
        empty_in_process["frameTree"]["childFrames"][0]["childFrames"] = json!([]);
        assert!(verify(&empty_in_process, None).is_ok());
        for (field, value) in [
            ("id", "other"),
            ("parentId", "other"),
            ("url", "https://zest-html.invalid/token/document?extra=1"),
            ("url", "file:///etc/passwd"),
            ("url", "https://example.com/"),
        ] {
            let mut changed = child.clone();
            changed["frameTree"]["frame"][field] = value.into();
            assert!(verify(&main, Some(&changed)).is_err());
            let mut changed_main = in_process.clone();
            changed_main["frameTree"]["childFrames"][0]["frame"] =
                changed["frameTree"]["frame"].clone();
            assert!(verify(&changed_main, None).is_err());
        }
        let mut changed_main = main;
        changed_main["frameTree"]["frame"]["url"] = "https://example.com/".into();
        assert!(verify(&changed_main, Some(&child)).is_err());
    }

    #[test]
    fn profile_removal_unlinks_directories_and_handles_readonly_browser_files() {
        let temporary = tempfile::tempdir().unwrap();
        let profile = temporary.path().join("profile");
        std::fs::create_dir_all(profile.join("cache")).unwrap();
        let file = profile.join("cache/data");
        std::fs::write(&file, b"disposable browser data").unwrap();
        #[cfg(windows)]
        {
            let mut permissions = std::fs::metadata(&file).unwrap().permissions();
            permissions.set_readonly(true);
            std::fs::set_permissions(&file, permissions).unwrap();
        }
        #[cfg(unix)]
        {
            let external = temporary.path().join("keep");
            std::fs::create_dir(&external).unwrap();
            std::fs::write(external.join("data"), b"keep").unwrap();
            std::os::unix::fs::symlink(&external, profile.join("external")).unwrap();
        }
        remove_profile(&profile).unwrap();
        assert!(!profile.exists());
        #[cfg(unix)]
        assert_eq!(
            std::fs::read(temporary.path().join("keep/data")).unwrap(),
            b"keep"
        );
    }

    #[tokio::test]
    async fn unavailable_engine_never_returns_a_fake_preview() {
        let previewer = HtmlPreviewer { browser: None };
        assert!(previewer
            .preview(&document("<h1>hello</h1>"))
            .await
            .unwrap_err()
            .contains("unavailable"));
    }

    #[tokio::test]
    #[ignore = "opt-in: launches an already installed Chromium/Edge browser"]
    async fn installed_browser_produces_png_and_reports_confined_script_behavior() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let canary = format!("http://{}/private", listener.local_addr().unwrap());
        let source = format!(
            r#"<style>body{{background:#123456;color:white}}</style><button id="count">0</button><script>
            const button=document.getElementById('count');button.onclick=()=>button.textContent=String(Number(button.textContent)+1);button.click();
            console.log('counter',button.textContent);
            try{{parent.document;console.error('parent leaked')}}catch(e){{console.log('parent denied')}}
            console.log('host bridge',typeof window.__TAURI__,typeof window.__TAURI_INTERNALS__);
            console.log('RTC',typeof RTCPeerConnection);
            console.log('popup',window.open('{canary}')===null);
            try{{const worker=new Worker(URL.createObjectURL(new Blob(['postMessage("worker-ran");fetch("{canary}")'])));worker.onmessage=()=>console.error('worker leaked');worker.onerror=()=>console.log('worker denied')}}catch(e){{console.log('worker denied')}}
            fetch('{canary}').catch(()=>console.log('network denied'));
            fetch('file:///etc/passwd').catch(()=>console.log('file denied'));
            const image=new Image();image.src='{canary}';document.body.appendChild(image);
            setTimeout(()=>{{throw new Error('preview-diagnostic-canary')}},10);
            </script>"#
        );
        let report = HtmlPreviewer::installed()
            .preview(&document(&source))
            .await
            .unwrap();
        validate_png(&report.png_base64).unwrap();
        let diagnostics = report.diagnostics.join("\n");
        for expected in [
            "counter 1",
            "parent denied",
            "host bridge undefined undefined",
            "RTC undefined",
            "popup true",
            "worker denied",
            "network denied",
            "file denied",
            "preview-diagnostic-canary",
        ] {
            assert!(
                diagnostics.contains(expected),
                "missing {expected}: {diagnostics}"
            );
        }
        assert!(!diagnostics.contains("parent leaked"));
        assert!(!diagnostics.contains("worker leaked"));
        assert!(
            tokio::time::timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err(),
            "generated document reached network canary"
        );
    }

    #[tokio::test]
    #[ignore = "opt-in: launches an already installed Chromium/Edge browser"]
    async fn installed_browser_runaway_script_obeys_public_twenty_second_timeout() {
        let started = std::time::Instant::now();
        let error = HtmlPreviewer::installed()
            .preview(&document("<script>while(true){}</script>"))
            .await
            .unwrap_err();
        assert!(error.contains("timed out after 20 seconds"), "{error}");
        assert!(
            started.elapsed() < Duration::from_secs(25),
            "preview exceeded its cleanup allowance"
        );
    }

    #[tokio::test]
    #[ignore = "opt-in: launches an already installed Chromium/Edge browser"]
    async fn installed_browser_is_reaped_and_profile_removed_on_cancel_and_timeout() {
        let executable = installed_browser().expect("installed browser required");
        for cancel in [false, true] {
            let mut browser = BrowserProcess::start(&executable).unwrap();
            let profile = browser.profile.as_ref().unwrap().path().to_owned();
            let pid = browser.child.id();
            tokio::time::timeout(PREVIEW_TIMEOUT, browser.endpoint())
                .await
                .unwrap()
                .unwrap();
            let future = async move {
                let _browser = browser;
                std::future::pending::<()>().await;
            };
            if cancel {
                let task = tokio::spawn(future);
                tokio::task::yield_now().await;
                task.abort();
                assert!(task.await.unwrap_err().is_cancelled());
            } else {
                assert!(tokio::time::timeout(Duration::from_millis(20), future)
                    .await
                    .is_err());
            }
            assert!(!profile.exists(), "profile survived dropped preview future");
            assert_process_stopped(pid);
        }
    }

    fn assert_process_stopped(pid: u32) {
        #[cfg(windows)]
        {
            use windows_sys::Win32::Foundation::CloseHandle;
            use windows_sys::Win32::System::Threading::{
                GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
            };
            let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
            if !process.is_null() {
                let mut exit_code = 0;
                let queried = unsafe { GetExitCodeProcess(process, &mut exit_code) };
                unsafe {
                    CloseHandle(process);
                }
                assert_ne!(queried, 0);
                assert_ne!(
                    exit_code, 259,
                    "browser process survived dropped preview future"
                );
            }
        }
        #[cfg(unix)]
        assert!(!Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success());
    }
}
