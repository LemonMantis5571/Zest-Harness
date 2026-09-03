//! `zest serve` — loopback coordinator daemon with inbound MCP.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context};
use serde::Serialize;
use zest_coordinator::{DelegationCoordinator, TokioSpawner};
use zest_core::{Config, DelegationStore, Ledger};

mod mcp;

const MIN_TOKEN_CHARS: usize = 32;
const SCAN_INTERVAL_MS: u64 = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ServePolicy {
    Gated,
    Trusted,
}

impl ServePolicy {
    fn parse(value: &str) -> anyhow::Result<Self> {
        match value.trim() {
            "gated" => Ok(Self::Gated),
            "trusted" => Ok(Self::Trusted),
            other => bail!("unknown policy `{other}`; use gated or trusted"),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Gated => "gated",
            Self::Trusted => "trusted",
        }
    }

    fn is_trusted(self) -> bool {
        matches!(self, Self::Trusted)
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReadyLine {
    kind: &'static str,
    protocol: &'static str,
    pid: u32,
    project_root: String,
    mcp_url: String,
    health_url: String,
    policy: ServePolicy,
}

pub async fn run(args: Vec<String>) -> anyhow::Result<()> {
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "--help" | "-h"))
    {
        print_help();
        return Ok(());
    }

    let mut project = None;
    let mut port: u16 = 0;
    let mut policy = None;
    let mut rest = args.into_iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--project" => {
                project = Some(rest.next().context("`--project` requires a path")?);
            }
            "--port" => {
                let value = rest.next().context("`--port` requires a number")?;
                port = value
                    .parse()
                    .with_context(|| format!("invalid --port `{value}`"))?;
            }
            "--policy" => {
                let value = rest
                    .next()
                    .context("`--policy` requires gated or trusted")?;
                policy = Some(ServePolicy::parse(&value)?);
            }
            other => bail!("unknown serve option `{other}` (try: zest serve --help)"),
        }
    }
    let project = project.context("`--project PATH` is required")?;
    let policy = match policy {
        Some(policy) => policy,
        None => match std::env::var("ZEST_SERVE_POLICY") {
            Ok(value) => ServePolicy::parse(&value)?,
            Err(_) => ServePolicy::Gated,
        },
    };
    let token = require_token()?;
    let root = prepare_project(Path::new(&project))?;
    let _config = Config::find(&root).context("could not load zest.toml for this project")?;
    let _store = DelegationStore::open(&root).map_err(|error| anyhow::anyhow!(error))?;

    let coordinator = Arc::new(DelegationCoordinator::with_runtime(
        Arc::new(Mutex::new(Ledger::load())),
        Arc::new(TokioSpawner::current()),
        Arc::new(zest_coordinator::NoopNotifier),
    ));
    coordinator
        .ensure_lock(&root)
        .map_err(|error| anyhow::anyhow!(error))?;
    coordinator
        .reconcile(&root)
        .map_err(|error| anyhow::anyhow!(error))?;
    if policy.is_trusted() {
        coordinator
            .apply_ready_jobs(&root)
            .map_err(|error| anyhow::anyhow!(error))?;
    }

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .context("could not bind the loopback MCP port")?;
    let bound = listener
        .local_addr()
        .context("could not read bound address")?;
    let mcp_url = format!("http://127.0.0.1:{}/mcp", bound.port());
    let health_url = format!("http://127.0.0.1:{}/healthz", bound.port());
    let ready = ReadyLine {
        kind: "ready",
        protocol: "zest-serve-v1",
        pid: std::process::id(),
        project_root: display_root(&root),
        mcp_url,
        health_url,
        policy,
    };
    let mut stdout = std::io::stdout();
    serde_json::to_writer(&mut stdout, &ready)?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    eprintln!(
        "zest serve listening on 127.0.0.1:{} for {} ({})",
        bound.port(),
        display_root(&root),
        policy.as_str()
    );

    let scanner_root = root.clone();
    let scanner = coordinator.clone();
    let scan_task = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_millis(SCAN_INTERVAL_MS));
        loop {
            ticker.tick().await;
            if let Err(error) = scanner.reconcile(&scanner_root) {
                eprintln!("zest serve reconcile: {error}");
            }
            if policy.is_trusted() {
                if let Err(error) = scanner.apply_ready_jobs(&scanner_root) {
                    eprintln!("zest serve auto-apply: {error}");
                }
            }
        }
    });

    let app = mcp::router(token, coordinator.clone(), root.clone(), policy);
    let serve = axum::serve(listener, app).with_graceful_shutdown(shutdown_signal());
    let result = serve.await.context("MCP server stopped");
    scan_task.abort();
    coordinator.shutdown(&root).await;
    result
}

pub fn print_help() {
    println!(
        "\
zest serve — headless coordinator daemon

USAGE
  zest serve --project PATH [--port 0] [--policy gated|trusted]

OPTIONS
  --project PATH              Project root this process will own (required)
  --port N                    Loopback TCP port; 0 selects a free port
  --policy gated|trusted      gated (default): create waits for
                              delegation_approve, apply waits for
                              delegation_apply.
                              trusted: the token holder can run a card
                              through worker, review, and apply without
                              those extra calls.
  -h, --help                  Show this help

ENVIRONMENT
  ZEST_SERVE_TOKEN            Required bearer token, at least 32 characters.
                              Never passed on argv, written to the project, or
                              printed in readiness/logs.
  ZEST_SERVE_POLICY           gated or trusted, used when --policy is omitted.

The process binds 127.0.0.1 only and prints one JSON readiness line on stdout
when it is ready to accept authenticated MCP at POST /mcp. Diagnostics go to
stderr. There is still no generic shell tool. Trusted mode does not skip
fingerprint, scope, or git apply --check.

This is not `zest run --jsonl` (one deny-only turn) and not
`[agents.<id>].mode = \"headless\"` (an external worker CLI).
"
    );
}

fn require_token() -> anyhow::Result<String> {
    let token = std::env::var("ZEST_SERVE_TOKEN").map_err(|_| {
        anyhow::anyhow!("ZEST_SERVE_TOKEN is required and must not be passed on argv")
    })?;
    if token.chars().count() < MIN_TOKEN_CHARS {
        bail!("ZEST_SERVE_TOKEN must be at least {MIN_TOKEN_CHARS} characters");
    }
    if token.chars().any(char::is_whitespace) {
        bail!("ZEST_SERVE_TOKEN must not contain whitespace");
    }
    Ok(token)
}

fn prepare_project(project: &Path) -> anyhow::Result<PathBuf> {
    let meta = std::fs::metadata(project)
        .with_context(|| format!("project `{}` does not exist", project.display()))?;
    if !meta.is_dir() {
        bail!("project `{}` is not a directory", project.display());
    }
    let root = std::fs::canonicalize(project)
        .with_context(|| format!("could not resolve project `{}`", project.display()))?;
    let marker = root.join(".zest");
    std::fs::create_dir_all(&marker)
        .with_context(|| format!("project `{}` is not writable", root.display()))?;
    let probe = marker.join(".serve-write-probe");
    std::fs::write(&probe, b"ok")
        .with_context(|| format!("project `{}` is not writable", root.display()))?;
    let _ = std::fs::remove_file(&probe);
    Ok(root)
}

fn display_root(root: &Path) -> String {
    let text = root.to_string_lossy();
    text.strip_prefix(r"\\?\")
        .unwrap_or(&text)
        .replace('\\', "/")
}

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => {}
            _ = term.recv() => {}
        }
        return;
    }
    #[cfg(not(unix))]
    {
        let _ = ctrl_c.await;
    }
}
