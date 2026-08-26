//! Web search for the agent via DuckDuckGo HTML (no API key).

use async_trait::async_trait;
use regex::Regex;
use serde_json::{json, Value};
use std::sync::OnceLock;

use super::outcome::ToolOutcome;
use super::Tool;

const MAX_RESULTS: usize = 8;
const MAX_QUERY_CHARS: usize = 400;
const USER_AGENT: &str = "ZestCodingAgent/0.1 (+https://github.com/local/zest; research)";

pub struct WebSearch;

impl WebSearch {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WebSearch {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for WebSearch {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the public web for up-to-date information (docs, APIs, errors, news). \
         Use when project files are not enough. Returns titled links with short snippets. \
         Prefer project tools (grep, read_file) for codebase questions."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Max results to return (1–8, default 5)",
                    "minimum": 1,
                    "maximum": 8
                }
            },
            "required": ["query"]
        })
    }

    async fn run(&self, input: Value) -> Result<ToolOutcome, String> {
        let query = input
            .get("query")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "missing required field `query`".to_string())?;
        if query.chars().count() > MAX_QUERY_CHARS {
            return Err(format!("query too long (max {MAX_QUERY_CHARS} chars)"));
        }
        let max = input
            .get("max_results")
            .and_then(Value::as_u64)
            .unwrap_or(5)
            .clamp(1, MAX_RESULTS as u64) as usize;

        let results = search_duckduckgo(query, max).await?;

        if results.is_empty() {
            return Ok(ToolOutcome::text(format!("No web results for: {query}")));
        }

        let mut out = String::from("Web search results:\n");
        for (i, hit) in results.iter().enumerate() {
            out.push_str(&format!("\n{}. {}\n", i + 1, hit.title));
            if !hit.url.is_empty() {
                out.push_str(&format!("   {}\n", hit.url));
            }
            if !hit.snippet.is_empty() {
                out.push_str(&format!("   {}\n", hit.snippet));
            }
        }
        Ok(ToolOutcome::text(out))
    }
}

#[derive(Debug, Clone)]
struct SearchHit {
    title: String,
    url: String,
    snippet: String,
}

async fn search_duckduckgo(query: &str, max: usize) -> Result<Vec<SearchHit>, String> {
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;

    let resp = client
        .post("https://html.duckduckgo.com/html/")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(format!("q={}", urlencoding(query)))
        .send()
        .await
        .map_err(|e| format!("duckduckgo request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("duckduckgo HTTP {}", resp.status()));
    }
    let html = resp.text().await.map_err(|e| e.to_string())?;
    Ok(parse_ddg_html(&html, max))
}

fn parse_ddg_html(html: &str, max: usize) -> Vec<SearchHit> {
    static LINK_RE: OnceLock<Regex> = OnceLock::new();
    let link_re = LINK_RE.get_or_init(|| {
        Regex::new(r#"(?s)class="result__a"[^>]*href="([^"]+)"[^>]*>(.*?)</a>"#)
            .expect("link regex")
    });
    static SNIP_RE: OnceLock<Regex> = OnceLock::new();
    let snip_re = SNIP_RE.get_or_init(|| {
        Regex::new(r#"(?s)class="result__snippet"[^>]*>(.*?)</(?:a|td)>"#).expect("snip regex")
    });

    let mut hits = Vec::new();
    let snippets: Vec<String> = snip_re
        .captures_iter(html)
        .map(|c| strip_tags(&c[1]))
        .collect();

    for (i, cap) in link_re.captures_iter(html).enumerate() {
        if hits.len() >= max {
            break;
        }
        let href = decode_ddg_href(&cap[1]);
        let title = strip_tags(&cap[2]);
        if title.is_empty() || href.is_empty() {
            continue;
        }
        let snippet = snippets.get(i).cloned().unwrap_or_default();
        hits.push(SearchHit {
            title,
            url: href,
            snippet,
        });
    }
    hits
}

fn decode_ddg_href(href: &str) -> String {
    // Links look like //duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com&...
    if let Some(rest) = href.split("uddg=").nth(1) {
        let enc = rest.split('&').next().unwrap_or(rest);
        return urlencoding_decode(enc);
    }
    if href.starts_with("http://") || href.starts_with("https://") {
        return href.to_string();
    }
    if href.starts_with("//") {
        return format!("https:{href}");
    }
    href.to_string()
}

fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    html_unescape(&out)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

fn urlencoding(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.as_bytes() {
        match *b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn urlencoding_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let h = |c: u8| -> Option<u8> {
                    match c {
                        b'0'..=b'9' => Some(c - b'0'),
                        b'a'..=b'f' => Some(c - b'a' + 10),
                        b'A'..=b'F' => Some(c - b'A' + 10),
                        _ => None,
                    }
                };
                if let (Some(hi), Some(lo)) = (h(bytes[i + 1]), h(bytes[i + 2])) {
                    out.push((hi << 4) | lo);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sample_ddg_html() {
        let html = r#"
        <div class="result">
          <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fdocs&rut=x">Rust <b>docs</b></a>
          <a class="result__snippet">Official documentation for Rust.</a>
        </div>
        "#;
        let hits = parse_ddg_html(html, 5);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Rust docs");
        assert_eq!(hits[0].url, "https://example.com/docs");
        assert!(hits[0].snippet.contains("Official documentation"));
    }

    #[test]
    fn decode_plain_https() {
        assert_eq!(
            decode_ddg_href("https://example.com/a"),
            "https://example.com/a"
        );
    }
}
