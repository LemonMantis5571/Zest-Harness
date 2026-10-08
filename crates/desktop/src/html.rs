//! Transient, token-only HTML transport. Never resolves a filesystem path.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{
    http::{Request, Response, StatusCode},
    Manager, State, WebviewWindow,
};
use zest_core::html::{HtmlDocument, HTML_CSP};

const MAX_VIEWS: usize = 32;
const VIEW_LIFETIME: Duration = Duration::from_secs(30 * 60);

#[derive(Default)]
pub(crate) struct HtmlViews(Mutex<HashMap<String, (Instant, String)>>);

#[derive(Serialize)]
pub(crate) struct HtmlView {
    token: String,
    url: String,
}

impl HtmlViews {
    #[cfg(debug_assertions)]
    pub(crate) fn count(&self) -> usize {
        self.0.lock().map(|views| views.len()).unwrap_or(0)
    }
    fn prepare(&self, document: HtmlDocument) -> Result<HtmlView, String> {
        let mut views = self.0.lock().map_err(|_| "HTML view lock failed")?;
        views.retain(|_, (created, _)| created.elapsed() < VIEW_LIFETIME);
        if views.len() >= MAX_VIEWS {
            return Err("Too many open HTML views; close one first".into());
        }
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(|_| "HTML token generation failed")?;
        let token: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        views.insert(token.clone(), (Instant::now(), document.isolated_html()));
        let url = if cfg!(any(windows, target_os = "android")) {
            format!("http://zest-html.localhost/{token}")
        } else {
            format!("zest-html://localhost/{token}")
        };
        Ok(HtmlView { token, url })
    }

    fn release(&self, token: &str) {
        if let Ok(mut views) = self.0.lock() {
            views.remove(token);
        }
    }

    pub(crate) fn response(&self, request: Request<Vec<u8>>) -> Response<Vec<u8>> {
        let uri = request.uri();
        let token = uri.path().strip_prefix('/').unwrap_or("");
        let valid = request.method() == "GET"
            && uri.query().is_none()
            && matches!(uri.host(), Some("localhost" | "zest-html.localhost"))
            && token.len() == 64
            && token.bytes().all(|b| b.is_ascii_hexdigit());
        let body = if valid {
            self.0.lock().ok().and_then(|views| {
                views
                    .get(token)
                    .filter(|(created, _)| created.elapsed() < VIEW_LIFETIME)
                    .map(|(_, html)| html.clone())
            })
        } else {
            None
        };
        let status = if body.is_some() {
            StatusCode::OK
        } else {
            StatusCode::NOT_FOUND
        };
        Response::builder().status(status)
            .header("Content-Type", "text/html; charset=utf-8")
            .header("Content-Security-Policy", HTML_CSP)
            .header("Cache-Control", "no-store")
            .header("X-Content-Type-Options", "nosniff")
            .header("Referrer-Policy", "no-referrer")
            .header("Permissions-Policy", "camera=(), microphone=(), geolocation=(), payment=(), usb=(), clipboard-read=(), clipboard-write=()")
            .body(body.unwrap_or_default().into_bytes()).expect("constant HTML response headers")
    }
}

pub(crate) fn trusted_host<R: tauri::Runtime>(
    webview: &tauri::Webview<R>,
    headers: &tauri::http::HeaderMap,
) -> bool {
    if webview.label() != "main" {
        return false;
    }
    if let Some(origin) = headers.get("Origin") {
        let Ok(origin) = origin.to_str() else {
            return false;
        };
        if origin == "null" || origin.contains("zest-html") {
            return false;
        }
    }
    webview.url().is_ok_and(|url| {
        (url.scheme() == "tauri" && url.host_str() == Some("localhost"))
            || (matches!(url.scheme(), "http" | "https")
                && url.host_str() == Some("tauri.localhost"))
            || (cfg!(debug_assertions)
                && url.scheme() == "http"
                && url.host_str() == Some("127.0.0.1")
                && url.port() == Some(1420))
            || (cfg!(debug_assertions)
                && url.scheme() == "http"
                && url.host_str() == Some("zest-check.localhost"))
    })
}

#[tauri::command]
pub(crate) fn prepare_html_view(
    window: WebviewWindow,
    views: State<'_, HtmlViews>,
    document: HtmlDocument,
) -> Result<HtmlView, String> {
    if window.label() != "main" {
        return Err("HTML views require the trusted main window".into());
    }
    if !cfg!(windows) {
        return Err("Interactive HTML is currently verified only on Windows. Source and Save remain available.".into());
    }
    views.prepare(document)
}

#[tauri::command]
pub(crate) fn release_html_view(views: State<'_, HtmlViews>, token: String) {
    views.release(&token);
}

#[tauri::command]
pub(crate) fn save_html_document(document: HtmlDocument) -> Result<bool, String> {
    let safe_name = crate::sanitize_markdown_filename(document.title())
        .trim_end_matches(".md")
        .to_owned()
        + ".html";
    let Some(mut path) = rfd::FileDialog::new()
        .set_title("Save HTML — opens outside Zest's sandbox")
        .add_filter("HTML", &["html"])
        .set_file_name(safe_name)
        .save_file()
    else {
        return Ok(false);
    };
    path.set_extension("html");
    // The chosen file is an explicit user export, not a document-supplied path.
    let html = format!(
        "<!doctype html><meta http-equiv=\"Content-Security-Policy\" content=\"{}\">{}",
        HTML_CSP,
        document.isolated_html()
    );
    zest_core::atomic_write(&path, html.as_bytes()).map_err(|e| e.to_string())?;
    Ok(true)
}

pub(crate) fn install<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    builder
        .manage(HtmlViews::default())
        .register_uri_scheme_protocol("zest-html", |context, request| {
            context.app_handle().state::<HtmlViews>().response(request)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tokens_are_revocable_bounded_and_never_file_locators() {
        let views = HtmlViews::default();
        let doc = HtmlDocument::parse("Counter".into(), "<button>1</button>".into()).unwrap();
        let view = views.prepare(doc.clone()).unwrap();
        let url = format!("http://zest-html.localhost/{}", view.token);
        let response = views.response(Request::builder().uri(&url).body(Vec::new()).unwrap());
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["Content-Security-Policy"], HTML_CSP);
        // A local-file URL cannot even enter the HTTP protocol request path.
        assert!(Request::builder()
            .uri("file:///C:/secret")
            .body(Vec::<u8>::new())
            .is_err());
        for path in [
            "http://zest-html.localhost/../secret",
            "http://zest-html.localhost/%2e%2e/secret",
            "http://evil.localhost/",
        ] {
            assert_eq!(
                views
                    .response(Request::builder().uri(path).body(Vec::new()).unwrap())
                    .status(),
                StatusCode::NOT_FOUND
            );
        }
        views.release(&view.token);
        views.release(&view.token);
        assert_eq!(
            views
                .response(Request::builder().uri(&url).body(Vec::new()).unwrap())
                .status(),
            StatusCode::NOT_FOUND
        );
        for _ in 0..MAX_VIEWS {
            views.prepare(doc.clone()).unwrap();
        }
        assert!(views.prepare(doc).is_err());
    }
}
