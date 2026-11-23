//! Mermaid diagrams and TeX math from a chat reply, drawn by the bundled mermaid and KaTeX in
//! the sandboxed document window. The page is built here from the reply's source; the source is
//! data inside the page (JSON in a non-script block), never markup. The window serves the
//! libraries itself, so the document still has no network access.

use serde::{Deserialize, Serialize};
use zest_core::html::HtmlDocument;

/// Diagram source or the sum of a reply's math, whichever applies.
pub const MAX_SOURCE_BYTES: usize = 64 * 1024;
/// Math expressions drawn from one reply.
pub const MAX_EXPRESSIONS: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Visual {
    /// The source of one ```mermaid fence.
    Diagram { source: String },
    /// Every `$…$` and `$$…$$` expression in one reply, in order.
    Math { expressions: Vec<Expression> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expression {
    pub tex: String,
    pub display: bool,
}

impl Visual {
    pub fn diagram(source: &str) -> Option<Self> {
        let source = source.trim_end_matches(['\r', '\n']).to_owned();
        let visual = Visual::Diagram { source };
        visual.validate().ok().map(|()| visual)
    }

    pub fn math(mut expressions: Vec<Expression>) -> Option<Self> {
        expressions.retain(|e| !e.tex.trim().is_empty());
        expressions.truncate(MAX_EXPRESSIONS);
        let visual = Visual::Math { expressions };
        visual.validate().ok().map(|()| visual)
    }

    /// Refuses empty, oversized or NUL-bearing source. The window checks again after decoding.
    pub fn validate(&self) -> Result<(), String> {
        let (bytes, empty, nul) = match self {
            Visual::Diagram { source } => (source.len(), source.trim().is_empty(), source.contains('\0')),
            Visual::Math { expressions } => (
                expressions.iter().map(|e| e.tex.len()).sum(),
                expressions.is_empty() || expressions.len() > MAX_EXPRESSIONS,
                expressions.iter().any(|e| e.tex.contains('\0')),
            ),
        };
        if empty || nul || bytes > MAX_SOURCE_BYTES {
            return Err("A diagram or math block must be nonempty, contain no NUL and be at most 64 KiB".into());
        }
        Ok(())
    }

    pub fn title(&self) -> String {
        match self {
            Visual::Diagram { .. } => "Diagram".into(),
            Visual::Math { expressions } if expressions.len() == 1 => "Math".into(),
            Visual::Math { expressions } => format!("Math ({} expressions)", expressions.len()),
        }
    }

    /// What the card's Source button shows.
    pub fn source(&self) -> String {
        match self {
            Visual::Diagram { source } => source.clone(),
            Visual::Math { expressions } => expressions
                .iter()
                .map(|e| if e.display { format!("$${}$$", e.tex) } else { format!("${}$", e.tex) })
                .collect::<Vec<_>>()
                .join("\n\n"),
        }
    }

    /// The page the window serves as the document, with the isolation prelude of any HTML
    /// document. Library URLs are relative to the document, so they resolve under its token.
    pub fn page(&self) -> Result<String, String> {
        let (head, data, script) = match self {
            Visual::Diagram { source } => (
                r#"<script src="vendor/mermaid.min.js"></script>"#,
                json_data(source),
                DIAGRAM_SCRIPT,
            ),
            Visual::Math { expressions } => (
                r#"<link rel="stylesheet" href="vendor/katex.min.css"><script src="vendor/katex.min.js"></script>"#,
                json_data(expressions),
                MATH_SCRIPT,
            ),
        };
        let html = format!(
            r#"<title>{title}</title><style>{PAGE_STYLE}</style>{head}<main id="out"></main><script type="application/json" id="source">{data}</script><script>{script}</script>"#,
            title = self.title(),
        );
        Ok(HtmlDocument::parse(self.title(), html)?.isolated_html())
    }
}

/// JSON that cannot close the block it sits in: every `<` is escaped.
fn json_data(value: &impl Serialize) -> String {
    serde_json::to_string(value).expect("plain data serializes").replace('<', "\\u003c")
}

const PAGE_STYLE: &str = "body{margin:0;padding:20px;font:14px system-ui,sans-serif;background:#fff;color:#1f2328}main{display:flex;flex-direction:column;gap:16px;align-items:flex-start}.display{align-self:stretch;text-align:center}.error{color:#b42318;white-space:pre-wrap;font-family:ui-monospace,monospace}svg{max-width:100%;height:auto}";

const DIAGRAM_SCRIPT: &str = r#"(()=>{'use strict';const out=document.getElementById('out');const src=JSON.parse(document.getElementById('source').textContent);const fail=e=>{out.className='error';out.textContent='This diagram did not render: '+((e&&e.message)||e);};try{mermaid.initialize({startOnLoad:false,securityLevel:'strict',theme:'default'});mermaid.render('zest-diagram',src).then(r=>{out.innerHTML=r.svg;},fail);}catch(e){fail(e);}})();"#;

const MATH_SCRIPT: &str = r#"(()=>{'use strict';const out=document.getElementById('out');for(const e of JSON.parse(document.getElementById('source').textContent)){const el=document.createElement('div');el.className=e.display?'display':'inline';out.appendChild(el);try{katex.render(e.tex,el,{displayMode:e.display,throwOnError:false,trust:false,strict:'ignore',maxExpand:1000,maxSize:50});}catch(err){el.className='error';el.textContent=e.tex;}}})();"#;

/// The libraries the window serves under `/{token}/vendor/`, with their content types.
pub fn vendor_file(name: &str) -> Option<(&'static [u8], &'static str)> {
    macro_rules! asset {
        ($path:expr) => {
            include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/vendor/", $path)).as_slice()
        };
    }
    macro_rules! fonts {
        ($($font:literal),* $(,)?) => {
            match name {
                $(concat!("fonts/KaTeX_", $font, ".woff2") => Some((asset!(concat!("katex/fonts/KaTeX_", $font, ".woff2")), "font/woff2")),)*
                _ => None,
            }
        };
    }
    match name {
        "mermaid.min.js" => Some((asset!("mermaid/mermaid.min.js"), "text/javascript; charset=utf-8")),
        "katex.min.js" => Some((asset!("katex/katex.min.js"), "text/javascript; charset=utf-8")),
        "katex.min.css" => Some((asset!("katex/katex.min.css"), "text/css; charset=utf-8")),
        _ => fonts!(
            "AMS-Regular", "Caligraphic-Bold", "Caligraphic-Regular", "Fraktur-Bold", "Fraktur-Regular",
            "Main-Bold", "Main-BoldItalic", "Main-Italic", "Main-Regular", "Math-BoldItalic", "Math-Italic",
            "SansSerif-Bold", "SansSerif-Italic", "SansSerif-Regular", "Script-Regular", "Size1-Regular",
            "Size2-Regular", "Size3-Regular", "Size4-Regular", "Typewriter-Regular",
        ),
    }
}

/// The document policy: `HTML_CSP` plus scripts, styles and fonts from this window's own
/// library path. Still no network, frames, workers, forms or host bridge.
pub fn csp(vendor_base: &str) -> String {
    format!(
        "default-src 'none'; script-src 'unsafe-inline' {vendor_base}; style-src 'unsafe-inline' {vendor_base}; img-src data: blob:; font-src data: {vendor_base}; connect-src 'none'; frame-src 'none'; worker-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'; sandbox allow-scripts"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn source_cannot_break_out_of_its_data_block() {
        let visual = Visual::diagram("graph TD\nA[</script><script>alert(1)</script>] --> B").unwrap();
        let page = visual.page().unwrap();
        let data = page.split(r#"id="source">"#).nth(1).unwrap().split("</script>").next().unwrap();
        assert!(!data.contains('<'), "{data}");
        let decoded: String = serde_json::from_str(data).unwrap();
        assert!(decoded.contains("</script><script>alert(1)"));
        assert!(page.contains(r#"src="vendor/mermaid.min.js""#));
        assert!(page.find("RTCPeerConnection").unwrap() < page.find("mermaid.initialize").unwrap());
    }

    #[::core::prelude::v1::test]
    fn math_keeps_order_and_drops_blank_expressions() {
        let e = |tex: &str, display| Expression { tex: tex.into(), display };
        let visual = Visual::math(vec![e("x^2", false), e("  ", false), e(r"\int_0^1 f", true)]).unwrap();
        assert_eq!(visual.title(), "Math (2 expressions)");
        assert_eq!(visual.source(), "$x^2$\n\n$$\\int_0^1 f$$");
        assert!(visual.page().unwrap().contains("vendor/katex.min.css"));
        assert!(Visual::math(vec![e(" ", true)]).is_none());
    }

    #[::core::prelude::v1::test]
    fn oversized_or_nul_source_is_refused_after_decoding_too() {
        assert!(Visual::diagram(&"a".repeat(MAX_SOURCE_BYTES + 1)).is_none());
        assert!(Visual::diagram("graph TD\0").is_none());
        let decoded: Visual = serde_json::from_str(r#"{"kind":"diagram","source":"   "}"#).unwrap();
        assert!(decoded.validate().is_err());
        assert!(serde_json::from_str::<Visual>(r#"{"kind":"diagram","source":"x","extra":1}"#).is_err());
    }

    #[::core::prelude::v1::test]
    fn every_font_the_stylesheet_names_is_served() {
        let (css, kind) = vendor_file("katex.min.css").unwrap();
        assert_eq!(kind, "text/css; charset=utf-8");
        let css = std::str::from_utf8(css).unwrap();
        let mut names: Vec<&str> = css.split("url(").skip(1).filter_map(|s| s.split(')').next()).filter(|u| u.ends_with(".woff2")).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), 20);
        for name in names {
            assert_eq!(vendor_file(name).map(|f| f.1), Some("font/woff2"), "{name}");
        }
        assert!(vendor_file("../katex.min.js").is_none());
        assert!(vendor_file("mermaid.min.js").is_some_and(|f| f.0.len() > 1_000_000));
    }

    #[::core::prelude::v1::test]
    fn the_policy_adds_only_the_library_path() {
        let policy = csp("http://zest-artifact.localhost/abc/vendor/");
        assert!(policy.contains("script-src 'unsafe-inline' http://zest-artifact.localhost/abc/vendor/;"));
        assert!(policy.contains("connect-src 'none'"));
        assert!(policy.ends_with("sandbox allow-scripts"));
    }
}
