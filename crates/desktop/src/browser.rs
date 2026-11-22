//! Local browser-session adapter backed by a Tauri webview.
//!
//! This is intentionally small: one webview per parent chat, script-based DOM
//! inspection, and semantic locators. It does not introduce Playwright,
//! Electron, a websocket broker, or a second browser process.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{json, Value};
use tauri::{AppHandle, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tokio::sync::oneshot;
use tokio::time::{sleep, timeout};
use zest_core::{BrowserAction, BrowserAdapter, BrowserRequest};

const BROWSER_WIDTH: f64 = 1_080.0;
const BROWSER_HEIGHT: f64 = 760.0;
const DEFAULT_POLL_MS: u64 = 100;
use zest_core::tools::browser_dom::{decode_eval_result, page_action, wrap_result};

/// Process-level handle used to create browser windows for chat-scoped
/// adapters. It is attached during Tauri setup, after the app handle exists.
pub(crate) struct BrowserHost {
    app: Mutex<Option<AppHandle>>,
    next_session: AtomicU64,
}

impl BrowserHost {
    pub(crate) fn new() -> Self {
        Self {
            app: Mutex::new(None),
            next_session: AtomicU64::new(1),
        }
    }

    pub(crate) fn attach(&self, app: AppHandle) {
        if let Ok(mut guard) = self.app.lock() {
            *guard = Some(app);
        }
    }

    pub(crate) fn adapter(self: &Arc<Self>) -> Arc<dyn BrowserAdapter> {
        let sequence = self.next_session.fetch_add(1, Ordering::Relaxed);
        Arc::new(LocalBrowserAdapter {
            host: Arc::clone(self),
            label: format!("zest-browser-{sequence}"),
            window: Mutex::new(None),
            serial: tokio::sync::Mutex::new(()),
        })
    }

    fn app(&self) -> Result<AppHandle, String> {
        self.app
            .lock()
            .map_err(|_| "browser host state is unavailable".to_string())?
            .clone()
            .ok_or_else(|| "browser host is not initialized".to_string())
    }
}

struct LocalBrowserAdapter {
    host: Arc<BrowserHost>,
    label: String,
    window: Mutex<Option<WebviewWindow>>,
    /// Browser calls in one model round are otherwise allowed to run
    /// concurrently by the agent. A single page needs ordering, especially
    /// for type → press and navigate → snapshot sequences.
    serial: tokio::sync::Mutex<()>,
}

impl Drop for LocalBrowserAdapter {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.window.lock() {
            if let Some(window) = guard.take() {
                let _ = window.close();
            }
        }
    }
}

#[async_trait]
impl BrowserAdapter for LocalBrowserAdapter {
    async fn execute(&self, request: BrowserRequest) -> Result<Value, String> {
        let _serial = self.serial.lock().await;
        match request.action {
            BrowserAction::Open => self.open(&request).await,
            BrowserAction::Snapshot
            | BrowserAction::Click
            | BrowserAction::Type
            | BrowserAction::Press => {
                evaluate(
                    &self.require_window()?,
                    &page_action(&request)?,
                    request.timeout_ms(),
                )
                .await
            }
            BrowserAction::Wait => self.wait_for(&request).await,
        }
    }
}

impl LocalBrowserAdapter {
    async fn open(&self, request: &BrowserRequest) -> Result<Value, String> {
        let raw_url = request
            .url
            .as_deref()
            .ok_or_else(|| "browser `open` requires `url`".to_string())?;
        let url = validate_external_url(raw_url)?;

        let window = match self.current_window()? {
            Some(window) => {
                window
                    .navigate(url.clone())
                    .map_err(|error| format!("browser navigation failed: {error}"))?;
                let _ = window.show();
                window
            }
            None => {
                let app = self.host.app()?;
                let window =
                    WebviewWindowBuilder::new(&app, &self.label, WebviewUrl::External(url.clone()))
                        .title("Zest Browser")
                        .inner_size(BROWSER_WIDTH, BROWSER_HEIGHT)
                        // Keep redirects and link clicks inside the same
                        // explicit web schemes accepted by `browser open`.
                        .on_navigation(|url| matches!(url.scheme(), "http" | "https"))
                        .visible(true)
                        .build()
                        .map_err(|error| format!("could not create browser window: {error}"))?;
                self.store_window(window.clone())?;
                window
            }
        };

        self.wait_until_ready(&window, request.timeout_ms()).await?;
        Ok(json!({
            "ok": true,
            "action": "open",
            "url": window.url().map(|url| url.to_string()).unwrap_or_else(|_| raw_url.to_string())
        }))
    }

    async fn wait_for(&self, request: &BrowserRequest) -> Result<Value, String> {
        let window = self.require_window()?;
        let expression = page_action(request)?;
        let deadline = Instant::now() + Duration::from_millis(request.timeout_ms());
        loop {
            let result = evaluate(&window, &expression, request.timeout_ms()).await?;
            let ready = result
                .get("ready")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let found = result
                .get("found")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if (request.locator.is_some() && found) || (request.locator.is_none() && ready) {
                return Ok(result);
            }
            if Instant::now() >= deadline {
                return Err(if request.locator.is_some() {
                    "timed out waiting for the browser locator".into()
                } else {
                    "timed out waiting for the browser page".into()
                });
            }
            sleep(Duration::from_millis(DEFAULT_POLL_MS)).await;
        }
    }

    async fn wait_until_ready(
        &self,
        window: &WebviewWindow,
        timeout_ms: u64,
    ) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            let result = evaluate(
                window,
                &wrap_result("(() => ({ ready: document.readyState !== 'loading' }))()"),
                timeout_ms.min(1_000),
            )
            .await?;
            if result
                .get("ready")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err("timed out waiting for the browser page to load".into());
            }
            sleep(Duration::from_millis(DEFAULT_POLL_MS)).await;
        }
    }

    fn current_window(&self) -> Result<Option<WebviewWindow>, String> {
        self.window
            .lock()
            .map_err(|_| "browser window state is unavailable".to_string())
            .map(|guard| guard.clone())
    }

    fn require_window(&self) -> Result<WebviewWindow, String> {
        self.current_window()?
            .ok_or_else(|| "no browser page is open; call browser open first".into())
    }

    fn store_window(&self, window: WebviewWindow) -> Result<(), String> {
        self.window
            .lock()
            .map_err(|_| "browser window state is unavailable".to_string())
            .map(|mut guard| *guard = Some(window))
    }
}

fn validate_external_url(raw: &str) -> Result<Url, String> {
    let url = Url::parse(raw).map_err(|error| format!("invalid browser URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("browser only opens http and https URLs".into());
    }
    Ok(url)
}

async fn evaluate(
    window: &WebviewWindow,
    expression: &str,
    timeout_ms: u64,
) -> Result<Value, String> {
    let (sender, receiver) = oneshot::channel();
    let sender = Arc::new(Mutex::new(Some(sender)));
    window
        .eval_with_callback(expression.to_owned(), move |raw| {
            let result = decode_eval_result(&raw);
            if let Ok(mut guard) = sender.lock() {
                if let Some(sender) = guard.take() {
                    let _ = sender.send(result);
                }
            }
        })
        .map_err(|error| format!("browser script failed: {error}"))?;
    let result = timeout(Duration::from_millis(timeout_ms.max(1)), receiver)
        .await
        .map_err(|_| "timed out waiting for browser script".to_string())?
        .map_err(|_| "browser script callback was dropped".to_string())??;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_urls_are_limited_to_web_schemes() {
        assert!(validate_external_url("https://example.com").is_ok());
        assert!(validate_external_url("http://localhost:3000").is_ok());
        assert!(validate_external_url("file:///secret.txt").is_err());
        assert!(validate_external_url("javascript:alert(1)").is_err());
    }
}
