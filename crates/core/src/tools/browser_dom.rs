//! Fixed, semantic page actions shared by desktop browser hosts.
use super::browser::{BrowserAction, BrowserRequest};
use serde_json::Value;
const MAX_INTERACTIVE_ELEMENTS: usize = 120;

fn snapshot(request: &BrowserRequest) -> Result<String, String> {
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
    Ok(expression)
}

fn click(request: &BrowserRequest) -> Result<String, String> {
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
    Ok(locator_script(&expression))
}

fn type_text(request: &BrowserRequest) -> Result<String, String> {
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
    Ok(locator_script(&expression))
}

fn press(request: &BrowserRequest) -> Result<String, String> {
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
    Ok(locator_script(&expression))
}

fn wait(request: &BrowserRequest) -> Result<String, String> {
    let locator = request
        .locator
        .as_ref()
        .map(json_literal)
        .transpose()?
        .unwrap_or_else(|| "null".into());
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
    Ok(locator_script(&expression))
}

pub fn page_action(request: &BrowserRequest) -> Result<String, String> {
    let expression = match request.action {
        BrowserAction::Open => return Err("Navigation is handled by the webview".into()),
        BrowserAction::Click => click(request)?,
        BrowserAction::Type => type_text(request)?,
        BrowserAction::Snapshot => snapshot(request)?,
        BrowserAction::Press => press(request)?,
        BrowserAction::Wait => wait(request)?,
    };
    Ok(wrap_result(&expression))
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

pub fn wrap_result(expression: &str) -> String {
    format!(
        r#"(() => {{
            try {{ return {{ ok: true, value: ({expression}) }}; }}
            catch (error) {{ return {{ ok: false, error: String(error?.message || error) }}; }}
        }})()"#
    )
}

pub fn decode_eval_result(raw: &str) -> Result<Value, String> {
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
