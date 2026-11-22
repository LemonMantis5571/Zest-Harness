//! Quick investigation: attach a wry child webview to the GPUI window through its raw window
//! handle (`impl HasWindowHandle for gpui::Window`). Only compiled with `--features webview`.

#[cfg(feature = "webview")]
thread_local! {
    static WEBVIEW: std::cell::RefCell<Option<wry::WebView>> = const { std::cell::RefCell::new(None) };
}

#[cfg(feature = "webview")]
pub fn attach(window: &gpui::Window) {
    use wry::dpi::{LogicalPosition, LogicalSize};
    let html = "data:text/html,<body style='font:16px Segoe UI;background:%23fff8e1;margin:12px'><h3>wry child webview</h3><p>Hosted inside a GPUI window via HasWindowHandle.</p><button id=b onclick=\"this.textContent='clicked '+(++window.n||(window.n=1))\">click me</button><input placeholder='type here (IME test)' style='display:block;margin-top:12px;width:90%25'></body>";
    let built = wry::WebViewBuilder::new()
        .with_url(html)
        .with_bounds(wry::Rect {
            position: LogicalPosition::new(560, 90).into(),
            size: LogicalSize::new(400, 360).into(),
        })
        .build_as_child(window);
    match built {
        Ok(wv) => {
            println!("WEBVIEW attached: child webview created on the GPUI window handle");
            WEBVIEW.with(|w| *w.borrow_mut() = Some(wv));
        }
        Err(e) => println!("WEBVIEW FAILED: {e}"),
    }
}

#[cfg(not(feature = "webview"))]
pub fn attach(_window: &gpui::Window) {
    println!("WEBVIEW: build with --features webview");
}
