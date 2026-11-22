//! A separate system webview process. Only the desktop's private stdin can issue typed actions.
use crate::web_content::{ARTIFACT, BROWSER, Reply};
use serde_json::{Value, json};
use std::{
    borrow::Cow,
    io::{self, BufRead, Read, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tao::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    window::{Window, WindowBuilder},
};
use wry::{
    WebContext, WebView, WebViewBuilder,
    http::{Request, Response},
};
use zest_core::{
    BrowserAction, BrowserRequest,
    html::{HTML_CSP, HtmlDocument},
    tools::browser_dom::{decode_eval_result, page_action},
};

enum Command {
    Browser(BrowserRequest),
    Loaded(u64, String),
    Close,
}
const MAX_COMMAND: u64 = 4 * 1024 * 1024;
fn read_line(input: &mut impl BufRead) -> Result<Option<Vec<u8>>, String> {
    let mut line = Vec::new();
    input
        .take(MAX_COMMAND + 1)
        .read_until(b'\n', &mut line)
        .map_err(|e| e.to_string())?;
    if line.len() as u64 > MAX_COMMAND || (!line.is_empty() && line.last() != Some(&b'\n')) {
        return Err("Web window command exceeds its limit or is incomplete".into());
    }
    Ok((!line.is_empty()).then_some(line))
}
fn reply(result: Reply) {
    let mut stdout = io::stdout().lock();
    if serde_json::to_writer(&mut stdout, &result).is_ok() {
        let _ = stdout.write_all(b"\n");
        let _ = stdout.flush();
    }
}

pub fn dispatch() -> bool {
    let Some(mode) = std::env::args_os().nth(1) else {
        return false;
    };
    if mode != BROWSER && mode != ARTIFACT {
        return false;
    }
    if let Err(e) = run(mode == ARTIFACT) {
        reply(Err(e));
    }
    true
}

fn build(builder: WebViewBuilder<'_>, window: &Window) -> Result<WebView, String> {
    let builder = builder
        .with_devtools(false)
        .with_clipboard(false)
        .with_permission_handler(|_| wry::PermissionResponse::Deny)
        .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
        .with_download_started_handler(|_, _| false);
    #[cfg(windows)]
    let builder = {
        use wry::WebViewBuilderExtWindows;
        builder.with_default_context_menus(false)
    };
    #[cfg(target_os = "linux")]
    {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        builder
            .build_gtk(window.default_vbox().ok_or("Missing window container")?)
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "linux"))]
    builder.build(window).map_err(|e| e.to_string())
}

fn external_url(raw: &str) -> bool {
    url::Url::parse(raw)
        .is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.host().is_some())
}

fn run(artifact: bool) -> Result<(), String> {
    let document = if artifact {
        let line = read_line(&mut io::stdin().lock())?.ok_or("No HTML document supplied")?;
        Some(serde_json::from_slice::<HtmlDocument>(&line).map_err(|e| e.to_string())?)
    } else {
        None
    };
    let event_loop = EventLoopBuilder::<Command>::with_user_event().build();
    let window = WindowBuilder::new()
        .with_title(
            document
                .as_ref()
                .map_or("Zest Browser", HtmlDocument::title),
        )
        .with_inner_size(LogicalSize::new(1080., 760.))
        .build(&event_loop)
        .map_err(|e| e.to_string())?;
    let mut context = WebContext::new(Some(
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join("web-data"),
    ));
    let mut webview = None;
    if let Some(doc) = document {
        if !cfg!(windows) {
            return Err(
                "Interactive HTML currently requires Windows. Source and Save HTML are available."
                    .into(),
            );
        }
        let routes = Arc::new(ArtifactRoutes::new(&doc)?);
        let source = routes.clone();
        let nav = routes.clone();
        let wrapper_url = routes.wrapper_url.clone();
        let served = Arc::new(AtomicBool::new(false));
        let seen = served.clone();
        let first_load = AtomicBool::new(true);
        let builder = WebViewBuilder::new_with_web_context(&mut context)
            .with_custom_protocol("zest-artifact".into(), move |_, req| {
                let is_document = req.uri().path() == source.document_path;
                let response = source.response(req);
                if is_document && response.status().is_success() {
                    seen.store(true, Ordering::Release);
                }
                response
            })
            .with_navigation_handler(move |url| url == nav.wrapper_url || url == nav.document_url)
            .with_on_page_load_handler(move |event, url| {
                if matches!(event, wry::PageLoadEvent::Finished)
                    && url == wrapper_url
                    && first_load.swap(false, Ordering::AcqRel)
                {
                    if served.load(Ordering::Acquire) {
                        reply(Ok(json!({"ok":true})));
                    } else {
                        reply(Err("The isolated HTML frame failed to load".into()));
                    }
                }
            })
            .with_url(&routes.wrapper_url);
        webview = Some(build(builder, &window)?);
    }
    let proxy = event_loop.create_proxy();
    let input_proxy = proxy.clone();
    std::thread::spawn(move || {
        let mut input = io::stdin().lock();
        while let Ok(Some(line)) = read_line(&mut input) {
            if artifact {
                break;
            }
            let result = serde_json::from_slice::<Value>(&line)
                .map_err(|e| e.to_string())
                .and_then(|v| BrowserRequest::from_value(&v));
            match result {
                Ok(request) => {
                    if input_proxy.send_event(Command::Browser(request)).is_err() {
                        break;
                    }
                }
                Err(e) => reply(Err(e)),
            }
        }
        let _ = input_proxy.send_event(Command::Close);
    });
    let mut epoch = 0u64;
    let mut opening = None;
    event_loop.run(move |event, _, flow| {
        *flow = ControlFlow::Wait;
        match event {
            Event::UserEvent(Command::Close)
            | Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => *flow = ControlFlow::Exit,
            Event::UserEvent(Command::Loaded(id, url))
                if opening == Some(id) && external_url(&url) =>
            {
                opening = None;
                reply(Ok(json!({"ok":true,"action":"open","url":url})));
            }
            Event::UserEvent(Command::Browser(request)) => {
                if request.action == BrowserAction::Open {
                    let url = request.url.as_deref().unwrap_or_default();
                    if !external_url(url) {
                        reply(Err(
                            "Browser navigation requires an HTTP or HTTPS URL".into()
                        ));
                        return;
                    }
                    webview.take();
                    epoch += 1;
                    let id = epoch;
                    opening = Some(id);
                    let signal = proxy.clone();
                    let builder = WebViewBuilder::new_with_web_context(&mut context)
                        .with_url(url)
                        .with_navigation_handler(|url| external_url(&url))
                        .with_on_page_load_handler(move |event, url| {
                            if matches!(event, wry::PageLoadEvent::Finished) {
                                let _ = signal.send_event(Command::Loaded(id, url));
                            }
                        });
                    match build(builder, &window) {
                        Ok(view) => webview = Some(view),
                        Err(e) => {
                            opening = None;
                            reply(Err(e));
                        }
                    }
                    return;
                }
                let Some(view) = &webview else {
                    reply(Err("Open a browser page first".into()));
                    return;
                };
                let result = page_action(&request).and_then(|script| {
                    view.evaluate_script_with_callback(&script, |raw| {
                        reply(decode_eval_result(&raw))
                    })
                    .map_err(|e| e.to_string())
                });
                if let Err(e) = result {
                    reply(Err(e));
                }
            }
            _ => {}
        }
    })
}

struct ArtifactRoutes {
    wrapper_url: String,
    document_url: String,
    wrapper_path: String,
    document_path: String,
    wrapper: Vec<u8>,
    document: Vec<u8>,
    created: Instant,
}
impl ArtifactRoutes {
    fn new(doc: &HtmlDocument) -> Result<Self, String> {
        let mut random = [0u8; 32];
        getrandom::fill(&mut random).map_err(|e| e.to_string())?;
        let token: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let origin = if cfg!(windows) {
            "http://zest-artifact.localhost"
        } else {
            "zest-artifact://localhost"
        };
        let wrapper_path = format!("/{token}");
        let document_path = format!("{wrapper_path}/document");
        let wrapper_url = format!("{origin}{wrapper_path}");
        let document_url = format!("{origin}{document_path}");
        let wrapper=format!(r#"<!doctype html><meta charset="utf-8"><title>Interactive document</title><style>body{{margin:0;height:100vh;display:flex;flex-direction:column;font:14px system-ui;background:#fff}}header{{padding:4px}}iframe{{border:0;flex:1}}</style><header><a href="{wrapper_url}">Reset document</a></header><iframe title="Interactive document" sandbox="allow-scripts" referrerpolicy="no-referrer" src="{document_url}"></iframe>"#).into_bytes();
        Ok(Self {
            wrapper_url,
            document_url,
            wrapper_path,
            document_path,
            wrapper,
            document: doc.isolated_html().into_bytes(),
            created: Instant::now(),
        })
    }
    fn response(&self, request: Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
        let uri = request.uri();
        let allowed = request.method() == "GET"
            && uri.query().is_none()
            && matches!(uri.host(), Some("localhost" | "zest-artifact.localhost"))
            && self.created.elapsed() < Duration::from_secs(1800);
        let (status, policy, body) = if allowed && uri.path() == self.wrapper_path {
            (
                200,
                format!(
                    "default-src 'none'; style-src 'unsafe-inline'; frame-src {}; base-uri 'none'; form-action 'none'",
                    self.document_url
                ),
                self.wrapper.clone(),
            )
        } else if allowed && uri.path() == self.document_path {
            (200, HTML_CSP.to_owned(), self.document.clone())
        } else {
            (404, "default-src 'none'".into(), vec![])
        };
        Response::builder().status(status).header("Content-Type","text/html; charset=utf-8").header("Content-Security-Policy",policy)
            .header("Cache-Control","no-store").header("Referrer-Policy","no-referrer").header("X-Content-Type-Options","nosniff")
            .header("Permissions-Policy","camera=(), microphone=(), geolocation=(), payment=(), usb=(), clipboard-read=(), clipboard-write=()")
            .body(Cow::Owned(body)).expect("constant response headers")
    }
}
