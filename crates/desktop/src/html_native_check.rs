//! Opt-in debug-only real WebView2 verification without user config or chats.

use crate::html::{self, HtmlViews};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use tauri::{http::Response, Manager, State, WebviewUrl, WebviewWindowBuilder};
use zest_core::html::HTML_CSP;

const PROBES: &[&str] = &[
    "parentRead",
    "parentWrite",
    "parentInvoke",
    "sameOriginSibling",
    "nestedRealm",
    "storage",
    "popup",
    "topNavigation",
    "parentNavigation",
    "worker",
    "sharedWorker",
    "form",
    "beacon",
    "foreignMessages",
    "rawNativeInvoke",
    "fetch",
    "xhr",
    "websocket",
    "eventsource",
    "image",
    "script",
    "stylesheet",
    "frame",
    "object",
    "localFetch",
    "localXhr",
    "localFrame",
    "localScript",
    "localImage",
    "ipcFetch",
    "bridgeInvoke",
];
const HOST_CHECKS: &[&str] = &[
    "empty_registry",
    "prepared_two_views",
    "sandbox_attributes",
    "foreign_messages",
    "artifact_dispatch_ignored",
    "wrong_key_dispatch_ignored",
    "release_idempotent",
    "main_host_unchanged",
];

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProbeResults {
    phase: String,
    violations: Vec<String>,
    attempted: Vec<String>,
    probes: BTreeMap<String, String>,
    rtc_before: bool,
    rtc_after: bool,
    canary_executed: bool,
    counter_value: u8,
    user_activation_at_probe: bool,
    document_still_protocol: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HostReport {
    checks: BTreeMap<String, bool>,
    results: Option<ProbeResults>,
    failed_step: Option<String>,
}

impl HostReport {
    fn valid(&self) -> bool {
        self.checks.len() == HOST_CHECKS.len()
            && self
                .checks
                .keys()
                .all(|name| HOST_CHECKS.contains(&name.as_str()))
            && self.failed_step.as_deref().is_none_or(|step| {
                [
                    "initialization",
                    "prepare",
                    "artifact",
                    "dispatch",
                    "release",
                    "integrity",
                    "report",
                ]
                .contains(&step)
            })
            && self.results.as_ref().is_none_or(|result| {
                result.phase == "done"
                    && result.counter_value <= 2
                    && result.attempted.len() <= PROBES.len()
                    && result
                        .attempted
                        .iter()
                        .all(|name| PROBES.contains(&name.as_str()))
                    && result.probes.len() <= PROBES.len()
                    && result.probes.iter().all(|(name, value)| {
                        PROBES.contains(&name.as_str())
                            && [
                                "SecurityError",
                                "TypeError",
                                "NotAllowedError",
                                "InvalidStateError",
                                "Error",
                                "denied",
                                "pending",
                                "resolved",
                                "accessible",
                                "rtcRecovered",
                                "rtcBlocked",
                                "blocked",
                                "opened",
                                "accepted",
                                "created",
                                "absent",
                                "attempted",
                                "sent",
                                "unexpected",
                            ]
                            .contains(&value.as_str())
                    })
                    && result.violations.len() <= 16
                    && result.violations.iter().all(|name| {
                        [
                            "default-src",
                            "connect-src",
                            "frame-src",
                            "object-src",
                            "worker-src",
                            "img-src",
                            "script-src",
                            "script-src-elem",
                            "style-src",
                            "style-src-elem",
                            "form-action",
                            "base-uri",
                            "font-src",
                            "sandbox",
                        ]
                        .contains(&name.as_str())
                    })
            })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProtocolObservation {
    status: u16,
    empty_body: bool,
    strict_csp: bool,
    no_store: bool,
    nosniff: bool,
    previously_served: bool,
}

#[derive(Default)]
struct ProtocolEvidence {
    served: HashSet<String>,
    observations: Vec<ProtocolObservation>,
    overflow: bool,
}

struct NativeCheck {
    report: PathBuf,
    protocol: Mutex<ProtocolEvidence>,
    artifact_invokes_denied: AtomicUsize,
    prepare_invokes_seen: AtomicUsize,
}

// Only the guarded main host can submit this closed, content-free schema. The
// output path is fixed by the validated private working directory, never IPC.
#[tauri::command]
fn html_check_report(
    check: State<'_, NativeCheck>,
    report: HostReport,
) -> Result<(), &'static str> {
    if !report.valid() {
        return Err("Invalid native check report");
    }
    let evidence = check
        .protocol
        .lock()
        .map_err(|_| "Native observer unavailable")?;
    let bytes = serde_json::to_vec(&serde_json::json!({
        "host": report,
        "protocol": evidence.observations,
        "protocolOverflow": evidence.overflow,
        "artifactInvokesDenied": check.artifact_invokes_denied.load(Ordering::Relaxed),
        "prepareInvokesSeen": check.prepare_invokes_seen.load(Ordering::Relaxed),
    }))
    .map_err(|_| "Native report serialization failed")?;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&check.report)
        .map_err(|_| "Native report output unavailable")?;
    output
        .write_all(&bytes)
        .map_err(|_| "Native report write failed")
}

#[tauri::command]
fn html_check_count(views: State<'_, HtmlViews>) -> usize {
    views.count()
}

fn startup_stage(file: &Path, stage: &str) {
    if let Ok(mut output) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
    {
        let _ = writeln!(output, "{stage}");
    }
}

pub(crate) fn run() {
    let port = std::env::var("ZEST_HTML_CHECK_DEBUG_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .expect("native check requires a loopback debugging port (zero enables discovery)");
    let directory = std::env::current_dir().expect("native check working directory");
    let profile = PathBuf::from(
        std::env::var_os("WEBVIEW2_USER_DATA_FOLDER")
            .expect("native check requires its owned WebView2 profile"),
    );
    assert!(
        directory.file_name().is_some_and(|name| name
            .to_str()
            .is_some_and(|name| name.starts_with("zest-html-native-check-")))
            && profile == directory.join("webview2"),
        "native check profile must belong to its private working directory"
    );
    let stages = directory.join("native-startup.stages");
    startup_stage(&stages, "private_directory_validated");
    let protocol_stages = stages.clone();
    let setup_stages = stages.clone();
    let driver = if std::env::var("ZEST_HTML_CHECK_SELF_DRIVER").as_deref() == Ok("1") {
        let input = directory.join("native-driver.js");
        assert!(
            std::fs::metadata(&input).is_ok_and(|metadata| metadata.len() <= 1024 * 1024),
            "native driver must be bounded"
        );
        std::fs::read_to_string(input).expect("owned native check driver")
    } else {
        String::new()
    };
    let browser_args = format!(
        "--remote-debugging-address=127.0.0.1 --remote-debugging-port={port} --no-first-run --no-default-browser-check --enable-logging --log-file=\"{}\"",
        directory.join("browser-startup.log").display()
    );
    let mut context = tauri::generate_context!();
    startup_stage(&stages, "context_created");
    context.config_mut().app.windows.clear();
    let host_csp = "default-src 'none'; script-src 'self'; connect-src ipc: http://ipc.localhost; style-src 'unsafe-inline'; frame-src http://zest-html.localhost zest-html:; object-src 'none'; base-uri 'none'; form-action 'none'";
    let app = tauri::Builder::default()
        .manage(HtmlViews::default())
        .manage(NativeCheck {
            report: directory.join("native-result.json"), protocol: Mutex::default(),
            artifact_invokes_denied: AtomicUsize::new(0),
            prepare_invokes_seen: AtomicUsize::new(0),
        })
        .register_uri_scheme_protocol("zest-html", |context, request| {
            let path = request.uri().path().to_owned();
            let response = context.app_handle().state::<HtmlViews>().response(request);
            let check = context.app_handle().state::<NativeCheck>();
            if let Ok(mut evidence) = check.protocol.lock() {
                let previously_served = evidence.served.contains(&path);
                if evidence.observations.len() < 8 {
                    evidence.observations.push(ProtocolObservation {
                        status: response.status().as_u16(),
                        empty_body: response.body().is_empty(),
                        strict_csp: response.headers().get("Content-Security-Policy").is_some_and(|header| header == HTML_CSP),
                        no_store: response.headers().get("Cache-Control").is_some_and(|header| header == "no-store"),
                        nosniff: response.headers().get("X-Content-Type-Options").is_some_and(|header| header == "nosniff"),
                        previously_served,
                    });
                    if response.status() == 200 { evidence.served.insert(path); }
                } else { evidence.overflow = true; }
            }
            response
        })
        .register_uri_scheme_protocol("zest-check", move |_, _| {
            startup_stage(&protocol_stages, "main_host_served");
            Response::builder().header("Content-Type", "text/html; charset=utf-8")
                .header("Content-Security-Policy", host_csp)
                .body(b"<!doctype html><title>Zest HTML native check</title><main id='host'>Native HTML check</main>".to_vec()).unwrap()
        })
        .invoke_handler(|invoke| {
            if invoke.message.command() == "prepare_html_view" {
                invoke.message.webview_ref().app_handle().state::<NativeCheck>()
                    .prepare_invokes_seen.fetch_add(1, Ordering::Relaxed);
            }
            if !html::trusted_host(invoke.message.webview_ref(), invoke.message.headers()) {
                if invoke.message.command() == "prepare_html_view" {
                    invoke.message.webview_ref().app_handle().state::<NativeCheck>()
                        .artifact_invokes_denied.fetch_add(1, Ordering::Relaxed);
                }
                invoke.resolver.reject("Commands require the trusted main host");
                return true;
            }
            let handler: fn(tauri::ipc::Invoke<tauri::Wry>) -> bool = tauri::generate_handler![html::prepare_html_view, html::release_html_view, html_check_count, html_check_report];
            handler(invoke)
        })
        .setup(move |app| {
            startup_stage(&setup_stages, "creating_webview");
            WebviewWindowBuilder::new(app, "main", WebviewUrl::External("http://zest-check.localhost/index.html".parse().unwrap()))
                .title("Zest HTML native check")
                .data_directory(profile.clone())
                .additional_browser_args(&browser_args)
                .devtools(true)
                .initialization_script(&driver)
                .inner_size(1024.0, 768.0)
                .focused(false)
                .visible(false)
                .build()?;
            startup_stage(&setup_stages, "webview_created");
            Ok(())
        })
        .build(context).expect("native HTML check runtime");
    startup_stage(&stages, "runtime_created");
    app.run(move |_, event| {
        if matches!(event, tauri::RunEvent::Ready) {
            startup_stage(&stages, "event_loop_ready");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_schema_rejects_content_and_unknown_fields() {
        let baseline = serde_json::json!({
            "checks": HOST_CHECKS.iter().map(|name| (name.to_string(), false)).collect::<BTreeMap<_, _>>(),
            "results": null,
            "failedStep": "initialization",
        });
        let report: HostReport = serde_json::from_value(baseline.clone()).unwrap();
        assert!(report.valid());
        let mut unknown = baseline.clone();
        unknown["token"] = serde_json::json!("must never enter the report");
        assert!(serde_json::from_value::<HostReport>(unknown).is_err());
        let mut unknown_check = baseline.clone();
        unknown_check["checks"]["documentContents"] = serde_json::json!(true);
        assert!(!serde_json::from_value::<HostReport>(unknown_check)
            .unwrap()
            .valid());
        let mut free_text = baseline;
        free_text["failedStep"] = serde_json::json!("arbitrary content");
        assert!(!serde_json::from_value::<HostReport>(free_text)
            .unwrap()
            .valid());
    }
}
