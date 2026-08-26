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
const MAX_INTERACTIVE_ELEMENTS: usize = 120;

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
            BrowserAction::Snapshot => self.snapshot(&request).await,
            BrowserAction::Click => self.click(&request).await,
            BrowserAction::Type => self.type_text(&request).await,
            BrowserAction::Press => self.press(&request).await,
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

    async fn snapshot(&self, request: &BrowserRequest) -> Result<Value, String> {
        let window = self.require_window()?;
        let limit = request.snapshot_limit();
        let expression = format!(
            r#"(() => {{
                const limit = {limit};
                const text = (document.body?.innerText || '').trim();
                const all = Array.from(document.querySelectorAll(
                    'a,button,input,textarea,select,[role],[contenteditable="true"]'
                ));
                const implicitRole = (el) => {{
                    if (el.getAttribute('role')) return el.getAttribute('role');
                    const tag = el.tagName.toLowerCase();
                    if (tag === 'a') return 'link';
                    if (tag === 'button') return 'button';
                    if (tag === 'textarea') return 'textbox';
                    if (tag === 'select') return 'combobox';
                    if (tag === 'input') return el.type === 'checkbox' ? 'checkbox' : 'textbox';
                    if (el.isContentEditable) return 'textbox';
                    return null;
                }};
                const accessibleName = (el) =>
                    el.getAttribute('aria-label') || el.getAttribute('name') ||
                    el.getAttribute('placeholder') || (el.innerText || el.textContent || '').trim();
                const visible = (el) => {{
                    const style = getComputedStyle(el);
                    return !el.hidden && style.display !== 'none' &&
                        style.visibility !== 'hidden' && el.getClientRects().length > 0;
                }};
                const interactive = all.filter(visible).slice(0, {MAX_INTERACTIVE_ELEMENTS}).map((el) => ({{
                    role: implicitRole(el),
                    name: accessibleName(el).slice(0, 240),
                    text: (el.innerText || el.textContent || '').trim().slice(0, 240),
                    tag: el.tagName.toLowerCase(),
                    disabled: Boolean(el.disabled),
                    href: el instanceof HTMLAnchorElement ? el.href : undefined
                }}));
                return {{
                    ok: true,
                    action: 'snapshot',
                    url: location.href,
                    title: document.title,
                    text: text.slice(0, limit),
                    truncated: text.length > limit,
                    interactive
                }};
            }})()"#
        );
        evaluate(&window, &expression, request.timeout_ms()).await
    }

    async fn click(&self, request: &BrowserRequest) -> Result<Value, String> {
        let window = self.require_window()?;
        let locator = request
            .locator
            .as_ref()
            .ok_or_else(|| "browser `click` requires `locator`".to_string())?;
        let locator = json_literal(locator)?;
        let expression = format!(
            r#"(() => {{
                const locator = {locator};
                const el = findElement(locator);
                if (!el) return {{ ok: false, error: 'no matching element for the locator' }};
                el.scrollIntoView({{ block: 'center', inline: 'center' }});
                el.focus({{ preventScroll: true }});
                el.click();
                return {{ ok: true, action: 'click', element: elementInfo(el) }};
            }})()"#
        );
        evaluate(&window, &locator_script(&expression), request.timeout_ms()).await
    }

    async fn type_text(&self, request: &BrowserRequest) -> Result<Value, String> {
        let window = self.require_window()?;
        let locator = request
            .locator
            .as_ref()
            .ok_or_else(|| "browser `type` requires `locator`".to_string())?;
        let text = request
            .text
            .as_deref()
            .ok_or_else(|| "browser `type` requires `text`".to_string())?;
        let locator = json_literal(locator)?;
        let text = json_literal(text)?;
        let expression = format!(
            r#"(() => {{
                const locator = {locator};
                const nextValue = {text};
                const el = findElement(locator);
                if (!el) return {{ ok: false, error: 'no matching element for the locator' }};
                el.scrollIntoView({{ block: 'center', inline: 'center' }});
                el.focus({{ preventScroll: true }});
                if (el.isContentEditable) {{
                    el.textContent = nextValue;
                }} else if ('value' in el) {{
                    const prototype = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
                    const setter = Object.getOwnPropertyDescriptor(prototype, 'value')?.set;
                    if (setter) setter.call(el, nextValue); else el.value = nextValue;
                }} else {{
                    return {{ ok: false, error: 'matched element is not editable' }};
                }}
                el.dispatchEvent(new Event('input', {{ bubbles: true }}));
                el.dispatchEvent(new Event('change', {{ bubbles: true }}));
                return {{ ok: true, action: 'type', element: elementInfo(el) }};
            }})()"#
        );
        evaluate(&window, &locator_script(&expression), request.timeout_ms()).await
    }

    async fn press(&self, request: &BrowserRequest) -> Result<Value, String> {
        let window = self.require_window()?;
        let key = request
            .key
            .as_deref()
            .ok_or_else(|| "browser `press` requires `key`".to_string())?;
        let key = json_literal(key)?;
        let locator = request.locator.as_ref().map(json_literal).transpose()?;
        let locator = locator.unwrap_or_else(|| "null".into());
        let expression = format!(
            r#"(() => {{
                const locator = {locator};
                const key = {key};
                const el = locator ? findElement(locator) : document.activeElement;
                if (!el) return {{ ok: false, error: 'no active element for the key press' }};
                el.focus({{ preventScroll: true }});
                const init = {{ key, code: key, bubbles: true, cancelable: true }};
                el.dispatchEvent(new KeyboardEvent('keydown', init));
                if (key === 'Enter' && el.form?.requestSubmit) el.form.requestSubmit();
                el.dispatchEvent(new KeyboardEvent('keyup', init));
                return {{ ok: true, action: 'press', key, element: elementInfo(el) }};
            }})()"#
        );
        evaluate(&window, &locator_script(&expression), request.timeout_ms()).await
    }

    async fn wait_for(&self, request: &BrowserRequest) -> Result<Value, String> {
        let window = self.require_window()?;
        let locator = request.locator.as_ref().map(json_literal).transpose()?;
        let locator = locator.unwrap_or_else(|| "null".into());
        let deadline = Instant::now() + Duration::from_millis(request.timeout_ms());
        loop {
            let expression = format!(
                r#"(() => {{
                    const locator = {locator};
                    const element = locator ? findElement(locator) : null;
                    return {{
                        ok: true,
                        action: 'wait',
                        ready: document.readyState !== 'loading',
                        found: Boolean(element),
                        element: element ? elementInfo(element) : null
                    }};
                }})()"#
            );
            let result =
                evaluate(&window, &locator_script(&expression), request.timeout_ms()).await?;
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
                "(() => ({ ready: document.readyState !== 'loading' }))()",
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

fn json_literal<T: serde::Serialize + ?Sized>(value: &T) -> Result<String, String> {
    serde_json::to_string(value).map_err(|error| format!("serialize browser input: {error}"))
}

/// The common locator helpers are injected only for actions that need them.
fn locator_script(expression: &str) -> String {
    format!(
        r#"(() => {{
            const roleFor = (el) => {{
                if (el.getAttribute('role')) return el.getAttribute('role').toLowerCase();
                const tag = el.tagName.toLowerCase();
                if (tag === 'a') return 'link';
                if (tag === 'button') return 'button';
                if (tag === 'textarea') return 'textbox';
                if (tag === 'select') return 'combobox';
                if (tag === 'input') return el.type === 'checkbox' ? 'checkbox' : 'textbox';
                if (el.isContentEditable) return 'textbox';
                return '';
            }};
            const accessibleName = (el) =>
                (el.getAttribute('aria-label') || el.getAttribute('name') ||
                    el.getAttribute('placeholder') || (el.innerText || el.textContent || '')).trim();
            const visible = (el) => {{
                const style = getComputedStyle(el);
                return !el.hidden && style.display !== 'none' &&
                    style.visibility !== 'hidden' && el.getClientRects().length > 0;
            }};
            const elementInfo = (el) => ({{
                tag: el.tagName.toLowerCase(),
                role: roleFor(el) || null,
                name: accessibleName(el).slice(0, 240),
            }});
            const findElement = (locator) => {{
                let candidates;
                try {{
                    candidates = locator.css
                        ? Array.from(document.querySelectorAll(locator.css))
                        : Array.from(document.querySelectorAll(
                            'a,button,input,textarea,select,[role],[contenteditable="true"]'
                        ));
                }} catch (error) {{
                    return null;
                }}
                candidates = candidates.filter(visible).filter((el) => {{
                    if (locator.role && roleFor(el) !== locator.role.toLowerCase()) return false;
                    const name = accessibleName(el).toLowerCase();
                    if (locator.name && !name.includes(locator.name.toLowerCase())) return false;
                    const text = (el.innerText || el.textContent || '').trim().toLowerCase();
                    if (locator.text && !text.includes(locator.text.toLowerCase())) return false;
                    return true;
                }});
                // Prefer the smallest text match, which avoids clicking a page
                // container when the locator names a nested button.
                if (!locator.css && locator.text) candidates.sort((a, b) =>
                    ((a.innerText || a.textContent || '').length - (b.innerText || b.textContent || '').length));
                return candidates[locator.index || 0] || null;
            }};
            return {expression};
        }})()"#
    )
}

async fn evaluate(
    window: &WebviewWindow,
    expression: &str,
    timeout_ms: u64,
) -> Result<Value, String> {
    let script = format!(
        r#"(() => {{
            try {{ return {{ ok: true, value: ({expression}) }}; }}
            catch (error) {{ return {{ ok: false, error: String(error?.message || error) }}; }}
        }})()"#
    );
    let (sender, receiver) = oneshot::channel();
    let sender = Arc::new(Mutex::new(Some(sender)));
    window
        .eval_with_callback(script, move |raw| {
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

fn decode_eval_result(raw: &str) -> Result<Value, String> {
    let mut value: Value = serde_json::from_str(raw.trim())
        .map_err(|error| format!("invalid browser script result: {error}"))?;
    // Some webview implementations serialize the callback payload one extra
    // time. Accept both forms so the adapter remains portable across Tauri's
    // Windows and macOS backends.
    if let Value::String(inner) = value {
        value = serde_json::from_str(&inner)
            .map_err(|error| format!("invalid nested browser script result: {error}"))?;
    }
    if value.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(value
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("browser script returned an error")
            .to_string());
    }
    Ok(value.get("value").cloned().unwrap_or(Value::Null))
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

    #[test]
    fn nested_callback_results_are_decoded() {
        let raw = serde_json::to_string(&serde_json::json!({
            "ok": true,
            "value": { "title": "fixture" }
        }))
        .unwrap();
        let decoded = decode_eval_result(&raw).unwrap();
        assert_eq!(decoded["title"], "fixture");
    }

    #[test]
    fn locator_script_keeps_locator_values_outside_script_code() {
        let expression = "(() => ({ ok: true }))()";
        let script = locator_script(expression);
        assert!(script.contains("findElement"));
        assert!(script.contains(expression));
    }
}
