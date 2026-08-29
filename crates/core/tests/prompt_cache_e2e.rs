//! Live prompt-cache check through the real provider and agent path.
//!
//! The usage screen's cache percentage is a window-wide merge of Zest's own
//! ledger **and** scanned Claude Code / Codex CLI transcripts. That headline
//! can sit at 7% even when this harness is caching well: the scanned history
//! is usually larger, and a lot of it is filed as fresh input. This test does
//! not touch `usage.json` or the scan cache. It writes a temp ledger, sends a
//! few short turns against a live provider, and asserts the **second** turn
//! actually reads from the provider cache.
//!
//! DeepSeek is the required target: it caches prefixes automatically and
//! reports hits as `prompt_cache_hit_tokens`, which the OpenAI-compatible
//! provider already splits out of `prompt_tokens`. Anthropic is run as well
//! when a key is present, because that path is where Zest places
//! `cache_control`.
//!
//!     cargo test -p zest-core --test prompt_cache_e2e -- --ignored --nocapture
//!
//! Credentials: OS store account `deepseek`, or `DEEPSEEK_API_KEY`. Anthropic
//! uses account `anthropic` or `ANTHROPIC_API_KEY`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use zest_core::provider::openai_compatible::OpenAiCompatibleProvider;
use zest_core::{
    credentials, load_env, Agent, AnthropicProvider, Config, Ledger, Prices, Provider,
    ProviderRegistry, SystemPrompt, ToolRegistry, DEFAULT_SYSTEM,
};

const DEEPSEEK_BASE: &str = "https://api.deepseek.com";
const DEEPSEEK_MODEL: &str = "deepseek-v4-flash";
const ANTHROPIC_MODEL: &str = "claude-haiku-5";

/// DeepSeek's documented floor is 64 tokens; V4 hits are more reliable near
/// 1k. The filler is stable on purpose: a timestamp here would bust the prefix
/// on every run.
fn cacheable_system() -> SystemPrompt {
    let mut body = DEFAULT_SYSTEM.to_string();
    body.push_str("\n\n");
    let filler = "Stable cache prefix block. Keep this text byte-identical across turns.\n";
    while body.len() < 5_000 {
        body.push_str(filler);
    }
    body.push_str("\nDo not use tools. Reply with the one requested word and nothing else.\n");
    SystemPrompt::new(body).with_volatile("e2e-cache-test workspace")
}

fn first_secret(account: &str, env: &str) -> Option<String> {
    credentials::get(account)
        .ok()
        .flatten()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            std::env::var(env)
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
}

/// Prefer the user's configured DeepSeek entry so the test spends against the
/// same account and model the desktop would. Fall back to the official host
/// when that provider is commented out of `zest.toml`.
fn deepseek_provider() -> Option<Arc<dyn Provider>> {
    let config = Config::find(".").ok()?;
    let (registry, skipped) = ProviderRegistry::from_config(&config);
    if let Some(provider) = registry.get("deepseek") {
        return Some(provider);
    }
    if let Some(skip) = skipped.iter().find(|row| row.id == "deepseek") {
        eprintln!("configured deepseek skipped: {}", skip.reason);
    }

    let key = first_secret("deepseek", "DEEPSEEK_API_KEY")?;
    OpenAiCompatibleProvider::new("deepseek", key, DEEPSEEK_BASE, DEEPSEEK_MODEL)
        .ok()
        .map(|provider| Arc::new(provider) as Arc<dyn Provider>)
}

fn anthropic_provider() -> Option<Arc<dyn Provider>> {
    let config = Config::find(".").ok()?;
    let (registry, _) = ProviderRegistry::from_config(&config);
    if let Some(provider) = registry.get("anthropic") {
        return Some(provider);
    }

    let key = first_secret("anthropic", "ANTHROPIC_API_KEY")?;
    AnthropicProvider::new("anthropic".into(), key, ANTHROPIC_MODEL.into(), Vec::new())
        .ok()
        .map(|provider| Arc::new(provider) as Arc<dyn Provider>)
}

struct TurnSnap {
    n: usize,
    prompt: u64,
    fresh: u32,
    cache_read: u32,
    cache_write: u32,
}

impl TurnSnap {
    fn hit_percent(&self) -> f64 {
        if self.prompt == 0 {
            0.0
        } else {
            f64::from(self.cache_read) / self.prompt as f64 * 100.0
        }
    }
}

async fn run_session(provider: Arc<dyn Provider>) -> (Vec<TurnSnap>, f64) {
    let dir = tempfile::tempdir().expect("temp ledger dir");
    let ledger = Arc::new(Mutex::new(Ledger::load_from(dir.path().join("usage.json"))));
    let mut agent = Agent::new(provider, ToolRegistry::new())
        .with_system(cacheable_system())
        .with_ledger(ledger.clone());
    // Short replies. High enough that Anthropic thinking can finish inside the
    // same budget the agent always requests.
    agent.max_tokens = 1_024;

    let prompts = [
        "Reply with the single word alpha.",
        "Reply with the single word beta.",
        "Reply with the single word gamma.",
    ];
    let mut snaps = Vec::new();

    for (i, prompt) in prompts.iter().enumerate() {
        if i == 1 {
            // DeepSeek builds the cache in the background on the first miss.
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        let mut sink = |_: zest_core::StreamEvent<'_>| {};
        agent
            .send(prompt, &mut sink)
            .await
            .unwrap_or_else(|error| panic!("turn {} failed: {error}", i + 1));
        let usage = agent
            .turn_usage()
            .unwrap_or_else(|| panic!("turn {} reported no usage summary", i + 1));
        assert!(
            usage.usage_available,
            "turn {} returned no provider usage — cannot measure cache",
            i + 1
        );
        snaps.push(TurnSnap {
            n: i + 1,
            prompt: usage.usage.prompt_tokens(),
            fresh: usage.usage.input_tokens,
            cache_read: usage.usage.cache_read_input_tokens,
            cache_write: usage.usage.cache_creation_input_tokens,
        });
    }

    let report = ledger
        .lock()
        .expect("ledger")
        .report(7, &Prices::default(), None);
    (snaps, report.totals.served_from_cache_percent)
}

fn print_session(label: &str, snaps: &[TurnSnap], session_hit: f64) {
    println!("--- {label} ---");
    for snap in snaps {
        println!(
            "turn {}: prompt={} fresh={} cache_read={} cache_write={} hit={:.1}%",
            snap.n,
            snap.prompt,
            snap.fresh,
            snap.cache_read,
            snap.cache_write,
            snap.hit_percent()
        );
    }
    println!("session (Zest-only ledger, no CLI scan): {session_hit:.1}% served from cache");
}

fn assert_prefix_was_cached(label: &str, snaps: &[TurnSnap], session_hit: f64) {
    assert!(snaps.len() >= 2, "{label}: expected at least two turns");
    assert!(
        snaps[0].prompt >= 200,
        "{label}: first-turn prompt was only {} tokens; the prefix is too short to cache",
        snaps[0].prompt
    );

    let warm = &snaps[1..];
    let best = warm
        .iter()
        .max_by(|a, b| {
            a.hit_percent()
                .partial_cmp(&b.hit_percent())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .expect("warm turn");
    assert!(
        best.cache_read > 0,
        "{label}: no later turn reported cache_read tokens. The harness sent a stable prefix but the provider did not serve it from cache.\n{snaps:?} session={session_hit:.1}%"
    );
    assert!(
        best.hit_percent() >= 40.0,
        "{label}: best later-turn hit rate was {:.1}% (cache_read={}, prompt={}). A working prefix cache should cover most of the prompt after the first turn.",
        best.hit_percent(),
        best.cache_read,
        best.prompt
    );
    assert!(
        session_hit >= 20.0,
        "{label}: isolated three-turn session hit rate was {session_hit:.1}%. That is the number to compare against the usage screen, not the merged window that includes CLI transcripts."
    );
}

#[tokio::test]
#[ignore = "calls a live provider; needs a DeepSeek key"]
async fn deepseek_reuses_a_stable_prompt_prefix() {
    load_env();
    let provider = deepseek_provider().expect(
        "DeepSeek is not configured. Save an API key under credential `deepseek` in Zest, \
         or set DEEPSEEK_API_KEY.",
    );
    let (snaps, session_hit) = run_session(provider).await;
    print_session("deepseek", &snaps, session_hit);
    assert_prefix_was_cached("deepseek", &snaps, session_hit);
}

#[tokio::test]
#[ignore = "calls a live provider; needs an Anthropic key"]
async fn anthropic_reuses_a_stable_prompt_prefix() {
    load_env();
    let Some(provider) = anthropic_provider() else {
        eprintln!("skipping anthropic cache e2e: no key");
        return;
    };
    let (snaps, session_hit) = run_session(provider).await;
    print_session("anthropic", &snaps, session_hit);
    assert_prefix_was_cached("anthropic", &snaps, session_hit);
}

impl std::fmt::Debug for TurnSnap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "turn {} prompt={} fresh={} read={} write={} hit={:.1}%",
            self.n,
            self.prompt,
            self.fresh,
            self.cache_read,
            self.cache_write,
            self.hit_percent()
        )
    }
}
