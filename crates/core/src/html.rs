//! Immutable, untrusted interactive documents and their isolated response policy.

use serde::{Deserialize, Deserializer, Serialize};

pub const MAX_HTML_BYTES: usize = 512 * 1024;
pub const HTML_CSP: &str = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data: blob:; font-src data:; connect-src 'none'; frame-src 'none'; worker-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'; sandbox allow-scripts";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HtmlDocument {
    title: String,
    html: String,
}

impl HtmlDocument {
    pub fn parse(title: String, html: String) -> Result<Self, String> {
        let title = title.trim().to_owned();
        if title.is_empty() || title.chars().count() > 120 || title.chars().any(char::is_control) {
            return Err("HTML title must be 1–120 characters without control characters".into());
        }
        if html.trim().is_empty() || html.len() > MAX_HTML_BYTES || html.contains('\0') {
            return Err("HTML must be nonempty, contain no NUL, and be at most 512 KiB".into());
        }
        Ok(Self { title, html })
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn source(&self) -> &str {
        &self.html
    }

    /// Must be delivered with HTML_CSP as a response header in a sandboxed frame.
    /// The prefix runs before any generated script. A document is not a host bridge.
    pub fn isolated_html(&self) -> String {
        format!("<!doctype html><meta charset=\"utf-8\"><meta name=\"referrer\" content=\"no-referrer\"><script>{}</script>{}", ISOLATION_BOOTSTRAP, self.html)
    }
}

impl<'de> Deserialize<'de> for HtmlDocument {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            title: String,
            html: String,
        }
        let raw = Raw::deserialize(deserializer)?;
        Self::parse(raw.title, raw.html).map_err(serde::de::Error::custom)
    }
}

const ISOLATION_BOOTSTRAP: &str = r#"(()=>{'use strict';for(const name of ['RTCPeerConnection','webkitRTCPeerConnection']){Object.defineProperty(globalThis,name,{value:undefined,writable:false,configurable:false});}})();"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_utf8_bytes_and_disk_decode_without_truncation() {
        let doc = HtmlDocument::parse("Counter".into(), "<button>1</button>".into()).unwrap();
        assert_eq!(
            serde_json::from_str::<HtmlDocument>(&serde_json::to_string(&doc).unwrap()).unwrap(),
            doc
        );
        assert!(HtmlDocument::parse("".into(), "x".into()).is_err());
        assert!(HtmlDocument::parse("x".into(), "é".repeat(MAX_HTML_BYTES / 2 + 1)).is_err());
        assert!(HtmlDocument::parse("x".into(), "\0".into()).is_err());
        assert!(serde_json::from_str::<HtmlDocument>(r#"{"title":"x","html":""}"#).is_err());
    }

    #[test]
    fn bootstrap_precedes_generated_code_without_rewriting_source() {
        let source = "<script>window.test = 1</script>";
        let doc = HtmlDocument::parse("Test".into(), source.into()).unwrap();
        assert!(doc.isolated_html().ends_with(source));
        assert!(
            doc.isolated_html().find("RTCPeerConnection").unwrap()
                < doc.isolated_html().find("window.test").unwrap()
        );
    }
}
