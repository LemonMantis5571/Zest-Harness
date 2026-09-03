use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

const TOKEN: &str = "zest-serve-e2e-token-32-chars-min";

struct Daemon {
    child: Child,
    mcp_url: String,
    policy: String,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn fixture_binary() -> &'static PathBuf {
    static FIXTURE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    FIXTURE.get_or_init(|| {
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("core")
            .join("tests")
            .join("fixtures")
            .join("external_agent_fixture.rs");
        let output = std::env::temp_dir().join(format!(
            "zest-serve-e2e-fixture-{}{}",
            std::process::id(),
            std::env::consts::EXE_SUFFIX
        ));
        let result = Command::new("rustc")
            .args(["--edition=2021"])
            .arg(&source)
            .arg("-o")
            .arg(&output)
            .output()
            .expect("start rustc for serve fixture");
        assert!(
            result.status.success(),
            "serve fixture compilation failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        output
    })
}

fn write_project(root: &Path) {
    git(root, &["init", "--quiet"]);
    git(root, &["config", "user.name", "Zest Test"]);
    git(root, &["config", "user.email", "zest-test@localhost"]);
    std::fs::write(root.join("README.md"), "serve e2e\n").unwrap();
    git(root, &["add", "."]);
    git(
        root,
        &["commit", "--quiet", "--no-verify", "-m", "baseline"],
    );
    write_worker_config(root);
}

fn write_worker_config(root: &Path) {
    let command = serde_json::to_string(&fixture_binary().to_string_lossy().to_string()).unwrap();
    std::fs::write(
        root.join("zest.toml"),
        format!(
            "[agents.worker]\nmode = \"headless\"\ncommand = {command}\nargs = [\"delegation\", \"{{prompt}}\"]\nworkspace = \"isolated\"\ntimeout_secs = 30\n"
        ),
    )
    .unwrap();
}

fn spawn_serve(root: &Path) -> Daemon {
    spawn_serve_with(root, &[])
}

fn spawn_serve_with(root: &Path, extra: &[&str]) -> Daemon {
    let mut args = vec![
        "serve".to_string(),
        "--project".to_string(),
        root.to_string_lossy().into_owned(),
        "--port".to_string(),
        "0".to_string(),
    ];
    args.extend(extra.iter().map(|arg| (*arg).to_string()));
    let mut child = Command::new(env!("CARGO_BIN_EXE_zest"))
        .args(&args)
        .env("ZEST_SERVE_TOKEN", TOKEN)
        .env_remove("ZEST_SERVE_POLICY")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn zest serve");
    let stdout = child.stdout.take().expect("serve stdout");
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .expect("read serve readiness line");
    let ready: Value = serde_json::from_str(line.trim()).unwrap_or_else(|error| {
        panic!("serve readiness was not JSON ({error}): {line}");
    });
    assert_eq!(ready["kind"], "ready");
    assert_eq!(ready["protocol"], "zest-serve-v1");
    let mcp_url = ready["mcpUrl"].as_str().expect("mcpUrl").to_string();
    let policy = ready["policy"].as_str().expect("policy").to_string();
    assert!(mcp_url.starts_with("http://127.0.0.1:"), "{mcp_url}");
    assert!(!line.contains(TOKEN), "token leaked into readiness");
    Daemon {
        child,
        mcp_url,
        policy,
    }
}

fn client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
}

fn rpc_response(
    daemon: &Daemon,
    token: &str,
    method: &str,
    params: Value,
) -> reqwest::blocking::Response {
    client()
        .post(&daemon.mcp_url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params
        }))
        .send()
        .unwrap()
}

fn rpc(daemon: &Daemon, method: &str, params: Value) -> Value {
    let response = rpc_response(daemon, TOKEN, method, params);
    assert_eq!(response.status(), 200, "{}", response.status());
    let body: Value = response.json().unwrap();
    if body.get("error").is_some() {
        panic!("MCP error for {method}: {body}");
    }
    body["result"].clone()
}

fn call_tool(daemon: &Daemon, name: &str, arguments: Value) -> Value {
    let result = rpc(
        daemon,
        "tools/call",
        json!({ "name": name, "arguments": arguments }),
    );
    let text = result["content"][0]["text"].as_str().expect("tool text");
    serde_json::from_str(text).unwrap_or_else(|error| panic!("tool JSON `{text}`: {error}"))
}

fn job_status(value: &Value) -> &str {
    value["status"].as_str().expect("status")
}

#[test]
fn serve_rejects_a_missing_token() {
    let temp = tempfile::tempdir().unwrap();
    write_project(temp.path());
    let output = Command::new(env!("CARGO_BIN_EXE_zest"))
        .args([
            "serve",
            "--project",
            &temp.path().to_string_lossy(),
            "--port",
            "0",
        ])
        .env_remove("ZEST_SERVE_TOKEN")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("ZEST_SERVE_TOKEN"), "{stderr}");
}

#[test]
fn serve_completes_create_approve_review_and_apply() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write_project(root);
    let daemon = spawn_serve(root);
    assert_eq!(daemon.policy, "gated");

    let unauthorized = client()
        .post(&daemon.mcp_url)
        .header("Content-Type", "application/json")
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}))
        .send()
        .unwrap();
    assert_eq!(unauthorized.status(), 401);

    let listed = rpc(&daemon, "tools/list", json!({}));
    let names: Vec<&str> = listed["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    for required in [
        "delegation_targets",
        "delegation_create",
        "delegation_list",
        "delegation_get",
        "delegation_artifact",
        "delegation_update",
        "delegation_approve",
        "delegation_retry",
        "delegation_cancel",
        "delegation_apply",
    ] {
        assert!(names.contains(&required), "missing {required} in {names:?}");
    }

    let created = call_tool(
        &daemon,
        "delegation_create",
        json!({
            "idempotencyKey": "e2e-1",
            "parentThreadId": "thread-e2e",
            "title": "Serve e2e",
            "objective": "Create the fixture change",
            "lane": "test",
            "scope": ["."],
            "worker": {"kind": "externalAgent", "agentId": "worker"}
        }),
    );
    assert_eq!(job_status(&created), "awaiting_approval");
    assert_eq!(created["approved"], false);
    let duplicate = call_tool(
        &daemon,
        "delegation_create",
        json!({
            "idempotencyKey": "e2e-1",
            "parentThreadId": "thread-e2e",
            "title": "Serve e2e",
            "objective": "Create the fixture change",
            "lane": "test",
            "scope": ["."],
            "worker": {"kind": "externalAgent", "agentId": "worker"}
        }),
    );
    assert_eq!(duplicate["jobId"], created["jobId"]);

    let approved = call_tool(
        &daemon,
        "delegation_approve",
        json!({
            "jobId": created["jobId"],
            "expectedUpdatedAt": created["updatedAt"]
        }),
    );
    assert_ne!(job_status(&approved), "awaiting_approval");

    let job_id = created["jobId"].as_str().unwrap().to_string();
    let deadline = Instant::now() + Duration::from_secs(20);
    let ready = loop {
        let job = call_tool(&daemon, "delegation_get", json!({ "jobId": job_id }));
        let status = job_status(&job).to_string();
        if status == "ready_to_apply" {
            break job;
        }
        assert!(
            !matches!(
                status.as_str(),
                "failed" | "blocked" | "cancelled" | "apply_conflict"
            ),
            "job ended in {status}: {job}"
        );
        assert!(Instant::now() < deadline, "timed out in {status}: {job}");
        std::thread::sleep(Duration::from_millis(100));
    };

    let diff = call_tool(
        &daemon,
        "delegation_artifact",
        json!({ "jobId": job_id, "name": "worker.diff", "offset": 0 }),
    );
    assert!(
        diff["content"]
            .as_str()
            .unwrap_or_default()
            .contains("delegated.txt"),
        "{diff}"
    );

    let applied = call_tool(
        &daemon,
        "delegation_apply",
        json!({
            "jobId": job_id,
            "expectedUpdatedAt": ready["updatedAt"]
        }),
    );
    assert_eq!(job_status(&applied), "accepted");
    let again = call_tool(
        &daemon,
        "delegation_apply",
        json!({
            "jobId": job_id,
            "expectedUpdatedAt": ready["updatedAt"]
        }),
    );
    assert_eq!(job_status(&again), "accepted");
    assert_eq!(
        std::fs::read_to_string(root.join("delegated.txt"))
            .unwrap()
            .replace("\r\n", "\n"),
        "fixture worker change\n"
    );
}

#[test]
fn serve_trusted_create_runs_and_applies_without_extra_calls() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write_project(root);
    let daemon = spawn_serve_with(root, &["--policy", "trusted"]);
    assert_eq!(daemon.policy, "trusted");

    let created = call_tool(
        &daemon,
        "delegation_create",
        json!({
            "idempotencyKey": "e2e-trusted-1",
            "parentThreadId": "thread-e2e",
            "title": "Trusted serve e2e",
            "objective": "Create the fixture change",
            "lane": "test",
            "scope": ["."],
            "worker": {"kind": "externalAgent", "agentId": "worker"}
        }),
    );
    assert_ne!(job_status(&created), "awaiting_approval");
    assert_eq!(created["approved"], true);

    let job_id = created["jobId"].as_str().unwrap().to_string();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let job = call_tool(&daemon, "delegation_get", json!({ "jobId": job_id }));
        let status = job_status(&job).to_string();
        if status == "accepted" {
            break;
        }
        assert!(
            !matches!(
                status.as_str(),
                "failed" | "blocked" | "cancelled" | "apply_conflict" | "changes_requested"
            ),
            "job ended in {status}: {job}"
        );
        assert!(Instant::now() < deadline, "timed out in {status}: {job}");
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(
        std::fs::read_to_string(root.join("delegated.txt"))
            .unwrap()
            .replace("\r\n", "\n"),
        "fixture worker change\n"
    );
}

#[test]
fn serve_without_init_rejects_a_missing_project() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("missing-app");
    let output = Command::new(env!("CARGO_BIN_EXE_zest"))
        .args([
            "serve",
            "--project",
            &missing.to_string_lossy(),
            "--port",
            "0",
        ])
        .env("ZEST_SERVE_TOKEN", TOKEN)
        .env_remove("ZEST_SERVE_POLICY")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--init"), "{stderr}");
}

#[test]
fn serve_init_bootstraps_git_and_trusted_apply() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("new-app");
    std::fs::create_dir_all(&root).unwrap();
    write_worker_config(&root);
    assert!(!root.join(".git").exists());
    let daemon = spawn_serve_with(&root, &["--init", "--policy", "trusted"]);
    assert_eq!(daemon.policy, "trusted");
    git(&root, &["rev-parse", "--verify", "HEAD"]);

    let created = call_tool(
        &daemon,
        "delegation_create",
        json!({
            "idempotencyKey": "e2e-init-1",
            "parentThreadId": "thread-e2e",
            "title": "Init serve e2e",
            "objective": "Create the fixture change",
            "lane": "test",
            "scope": ["."],
            "worker": {"kind": "externalAgent", "agentId": "worker"}
        }),
    );
    assert_ne!(job_status(&created), "awaiting_approval");
    let job_id = created["jobId"].as_str().unwrap().to_string();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let job = call_tool(&daemon, "delegation_get", json!({ "jobId": job_id }));
        let status = job_status(&job).to_string();
        if status == "accepted" {
            break;
        }
        assert!(
            !matches!(
                status.as_str(),
                "failed" | "blocked" | "cancelled" | "apply_conflict" | "changes_requested"
            ),
            "job ended in {status}: {job}"
        );
        assert!(Instant::now() < deadline, "timed out in {status}: {job}");
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(
        std::fs::read_to_string(root.join("delegated.txt"))
            .unwrap()
            .replace("\r\n", "\n"),
        "fixture worker change\n"
    );
}
