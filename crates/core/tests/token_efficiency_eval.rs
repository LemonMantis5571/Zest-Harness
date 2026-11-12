//! Repository-only, opt-in provider evaluation. Normal test runs never make
//! network calls or touch user transcripts. See the ignored test's guard and
//! environment variables before starting a paid run.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use zest_core::{
    ApprovalDecision, ApprovalMode, ApprovalPolicy, ApprovalRequest, Approver, Config, Ledger,
    McpCatalog, McpServerConfig, McpToolDef, Message, Prices, RateCatalog, RequestSourceEstimates,
    RuntimeBuilder, StreamEvent,
};

const FIXTURES: &str = include_str!("fixtures/token_efficiency_tasks.json");
const VARIANTS: &[&str] = &[];
const LARGE_OUTPUT_PAGE_LIMIT: usize = 40;

#[derive(Debug, Deserialize)]
struct FixtureCase {
    id: String,
    category: String,
    prompt: String,
    #[serde(default)]
    files: BTreeMap<String, String>,
    assertion: Assertion,
    large_lines: Option<usize>,
    large_marker_line: Option<usize>,
    large_page_offset: Option<usize>,
    large_marker: Option<String>,
    history_note: Option<String>,
    /// Fixture files the task may change. Every other fixture file must be
    /// byte-identical afterwards, and writes to it are denied.
    #[serde(default)]
    writable: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Assertion {
    /// `answer_contains`, `file_equals`, or `cargo_test_passes`.
    kind: String,
    path: Option<String>,
    #[serde(default)]
    value: String,
    /// Other acceptable values, e.g. a symbol for its spelled-out name.
    #[serde(default)]
    alternatives: Vec<String>,
    /// For exact tokens and labels, where letter case is part of the answer.
    #[serde(default)]
    case_sensitive: bool,
}

impl Assertion {
    fn expected(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.value.as_str()).chain(self.alternatives.iter().map(String::as_str))
    }
}

#[derive(Debug, Clone, Serialize)]
struct RequestBreakdown {
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    estimated_prompt_tokens: u64,
    provider_prompt_tokens: Option<u64>,
    prompt_estimate_delta_tokens: Option<i64>,
    /// Cache reads plus writes. They are part of the prompt the section
    /// estimates describe, but providers do not say which sections they cover.
    cache_tokens_not_split_by_section: u64,
    /// True when some cached tokens were priced at the input rate because no
    /// cache rate is known for the model. Cold/warm cost deltas are then not
    /// evidence of cache savings either way.
    cache_rate_assumed: bool,
    source_estimates: RequestSourceEstimates,
    elapsed_ms: u64,
    failed_requests: u64,
    usage_unavailable: u64,
    priced_percent: f64,
    api_equivalent_usd: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
struct TaskResult {
    case_id: String,
    category: String,
    variant: String,
    cache_condition: String,
    passed: bool,
    priming_passed: Option<bool>,
    provider_rounds: u64,
    priming_rounds: u64,
    latency_ms: u64,
    tool_errors: u64,
    priming_tool_errors: u64,
    api_equivalent_usd: Option<f64>,
    priming_api_equivalent_usd: Option<f64>,
    combined_api_equivalent_usd: Option<f64>,
    /// For warm runs, whether the provider reported any cache reads at all.
    warm_cache_read_observed: Option<bool>,
    breakdown: RequestBreakdown,
}

#[derive(Debug, Serialize)]
struct UnpairedRun {
    case_id: String,
    category: String,
    variant: String,
    phase: &'static str,
    outcome: &'static str,
    agent_succeeded: bool,
    assertion_passed: bool,
    provider_rounds: u64,
    latency_ms: u64,
    tool_errors: u64,
    breakdown: RequestBreakdown,
}

#[derive(Debug, Serialize)]
struct EvaluationReport {
    started_at_unix_secs: u64,
    provider_id: String,
    requested_model: Option<String>,
    requested_effort: Option<String>,
    variants: Vec<String>,
    cache_conditions: [&'static str; 2],
    optional_api_equivalent_limit_usd: Option<f64>,
    limit_note: &'static str,
    rate_source: &'static str,
    cache_condition_note: &'static str,
    rates_fetched_at_unix_secs: Option<u64>,
    rates_content_blake3: Option<String>,
    pricebook_revision: String,
    pricebook_content_blake3: Option<String>,
    /// Hash of the first run's cacheable system prompt. It includes personal
    /// skills from the home directory, so two machines' token counts are only
    /// comparable when this matches.
    system_prompt_blake3: Option<String>,
    results: Vec<TaskResult>,
    unpaired_runs: Vec<UnpairedRun>,
    paired_analysis: Vec<PairedAnalysis>,
    stopped_early: bool,
}

#[derive(Debug, Serialize)]
struct PairedAnalysis {
    variant: String,
    cache_condition: String,
    paired_tasks: usize,
    quality_regressions: usize,
    tool_error_regressions: usize,
    mean_cost_delta_usd: Option<f64>,
    confidence_95_low_usd: Option<f64>,
    confidence_95_high_usd: Option<f64>,
    fixture_promotion_evidence: bool,
}

#[derive(Debug, Clone)]
struct OneRun {
    passed: bool,
    agent_succeeded: bool,
    assertion_passed: bool,
    rounds: u64,
    latency_ms: u64,
    tool_errors: u64,
    breakdown: RequestBreakdown,
    system_prompt_blake3: Option<String>,
}

#[derive(Clone, Copy)]
struct FixtureRunSettings<'a> {
    base_config: &'a Config,
    provider_id: &'a str,
    model: Option<&'a str>,
    effort: Option<&'a str>,
    prices: &'a Prices,
}

#[test]
fn fixture_suite_has_twenty_cases_across_the_five_planned_categories() {
    let cases: Vec<FixtureCase> = serde_json::from_str(FIXTURES).unwrap();
    assert_eq!(cases.len(), 20);
    let mut counts = BTreeMap::new();
    for case in &cases {
        *counts.entry(case.category.as_str()).or_insert(0usize) += 1;
        if case.category == "large_output" {
            let offset = case.large_page_offset.expect("large-output page offset");
            let marker_line = case.large_marker_line.expect("large-output marker line");
            let min_bytes_before_window = offset.saturating_sub(1).saturating_mul(243); // generated record line's minimum byte length
            assert!(
                min_bytes_before_window > zest_core::tools::read_file::MAX_BYTES,
                "{} must page beyond read_file's retained-byte cap",
                case.id
            );
            assert!(
                (offset..offset + LARGE_OUTPUT_PAGE_LIMIT).contains(&marker_line),
                "{} marker must be inside the requested page",
                case.id
            );
        } else {
            assert!(case.large_page_offset.is_none());
        }
        match case.category.as_str() {
            "edit" => {
                assert_eq!(case.assertion.kind, "file_equals", "{}", case.id);
                let path = case.assertion.path.as_ref().expect("edit assertion path");
                assert_eq!(case.writable, vec![path.clone()], "{}", case.id);
                assert_ne!(
                    normalize_file(&case.files[path]),
                    normalize_file(&case.assertion.value),
                    "{} expects a change",
                    case.id
                );
            }
            "debugging" => {
                assert_eq!(case.assertion.kind, "cargo_test_passes", "{}", case.id);
                assert_eq!(
                    case.writable,
                    vec!["src/math.rs".to_string()],
                    "{}",
                    case.id
                );
            }
            _ => {
                assert_eq!(case.assertion.kind, "answer_contains", "{}", case.id);
                assert!(case.writable.is_empty(), "{}", case.id);
                assert!(!case.assertion.value.is_empty(), "{}", case.id);
            }
        }
    }
    assert_eq!(counts.get("search_answer"), Some(&4));
    assert_eq!(counts.get("edit"), Some(&4));
    assert_eq!(counts.get("debugging"), Some(&4));
    assert_eq!(counts.get("large_output"), Some(&4));
    assert_eq!(counts.get("long_history"), Some(&4));
}

#[test]
fn answer_matching_ignores_case_unless_asked_but_respects_word_boundaries() {
    assert!(answer_contains("Amber", "amber", false));
    assert!(answer_contains("The badge is AMBER.", "amber", false));
    assert!(!answer_contains("The badge is gold.", "amber", false));

    // Substrings of other words and numbers are not the answer.
    assert!(!answer_contains("Go, I trust.", "Rust", false));
    assert!(!answer_contains("The admiral owns it.", "Mira", false));
    assert!(!answer_contains("It is 5; usize is 64-bit.", "6", false));
    assert!(answer_contains("RETRY_LIMIT is 6.", "6", false));
    assert!(!answer_contains("about 6.5", "6", false));
    assert!(answer_contains("timeout_secs = 45s", "45", false));
    assert!(!answer_contains("145", "45", false));
    assert!(!answer_contains("ZEST_MARKER_10", "ZEST_MARKER_1", true));
    assert!(answer_contains("region: eu-west-2", "eu-west-2", false));

    // Exact tokens keep their case when the fixture says so.
    assert!(answer_contains("ZEST_MARKER_1", "ZEST_MARKER_1", true));
    assert!(!answer_contains("zest_marker_1", "ZEST_MARKER_1", true));
    assert!(answer_contains("Use Δ.", "Δ", false));
}

#[test]
fn debugging_fixtures_are_judged_by_their_tests_not_by_one_spelling_of_the_fix() {
    if std::process::Command::new("cargo")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }
    let cases: Vec<FixtureCase> = serde_json::from_str(FIXTURES).unwrap();
    let case = cases.iter().find(|case| case.id == "debug-clamp").unwrap();
    let temp = tempfile::tempdir().unwrap();
    reset_fixture(temp.path(), case).unwrap();
    assert!(
        !assertion_passes(temp.path(), case, "").unwrap(),
        "the bug must fail"
    );

    // A correct fix the old substring check rejected.
    std::fs::write(
        temp.path().join("src/math.rs"),
        "pub fn clamp_low(value: i32) -> i32 { std::cmp::max(value, 0) }\n",
    )
    .unwrap();
    assert!(assertion_passes(temp.path(), case, "").unwrap());

    // Weakening the tests instead of fixing the code does not pass.
    reset_fixture(temp.path(), case).unwrap();
    std::fs::write(temp.path().join("src/lib.rs"), "mod math;\n").unwrap();
    assert!(!assertion_passes(temp.path(), case, "").unwrap());
}

#[test]
fn edit_fixtures_require_the_rest_of_the_file_intact() {
    let cases: Vec<FixtureCase> = serde_json::from_str(FIXTURES).unwrap();
    let case = cases.iter().find(|case| case.id == "edit-version").unwrap();
    let temp = tempfile::tempdir().unwrap();
    reset_fixture(temp.path(), case).unwrap();

    std::fs::write(temp.path().join("app.txt"), "name = zest\nversion = 1.3\n").unwrap();
    assert!(assertion_passes(temp.path(), case, "").unwrap());
    std::fs::write(temp.path().join("app.txt"), "name = zest\r\nversion = 1.3").unwrap();
    assert!(
        assertion_passes(temp.path(), case, "").unwrap(),
        "line endings are not content"
    );

    // Appending the new line while keeping the old one is not the edit.
    std::fs::write(
        temp.path().join("app.txt"),
        "name = zest\nversion = 1.2\nversion = 1.3\n",
    )
    .unwrap();
    assert!(!assertion_passes(temp.path(), case, "").unwrap());
}

/// Paid live fixture test. It intentionally has no built-in spend ceiling: set
/// `ZEST_TOKEN_EVAL_LIMIT_USD` only if you want one. The estimate is checked
/// after each fixture run, so a limit can be exceeded by at most the in-flight
/// fixture task.
///
/// Required: `ZEST_TOKEN_EVAL=1` and `ZEST_TOKEN_EVAL_PROVIDER=<configured id>`.
/// Optional: `ZEST_TOKEN_EVAL_MODEL` and `ZEST_TOKEN_EVAL_EFFORT`.
/// Warm runs include a successful identical priming task; its cost is reported
/// separately and included in the combined task estimate.
#[tokio::test]
#[ignore = "opt-in live provider evaluation; makes paid requests"]
async fn live_task_cost_evaluation() {
    assert_eq!(std::env::var("ZEST_TOKEN_EVAL").as_deref(), Ok("1"));
    let provider_id = std::env::var("ZEST_TOKEN_EVAL_PROVIDER")
        .expect("set ZEST_TOKEN_EVAL_PROVIDER to a configured provider id");
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("core crate is under the workspace");
    let base_config = Config::find(workspace).expect("load existing provider configuration");
    assert!(
        base_config.providers.contains_key(&provider_id),
        "provider `{provider_id}` is not configured"
    );
    let requested_model = std::env::var("ZEST_TOKEN_EVAL_MODEL").ok();
    let requested_effort = std::env::var("ZEST_TOKEN_EVAL_EFFORT").ok();
    let limit = std::env::var("ZEST_TOKEN_EVAL_LIMIT_USD")
        .ok()
        .map(|value| value.parse::<f64>().expect("limit must be a USD number"));
    let variants = selected_variants();
    let cases: Vec<FixtureCase> = serde_json::from_str(FIXTURES).unwrap();
    let rates = RateCatalog::load();
    let pricebook = Prices::load();
    let prices = pricebook.clone().with_catalog(rates.clone());
    let run_settings = FixtureRunSettings {
        base_config: &base_config,
        provider_id: &provider_id,
        model: requested_model.as_deref(),
        effort: requested_effort.as_deref(),
        prices: &prices,
    };
    let rates_hash = RateCatalog::default_path()
        .and_then(|path| std::fs::read(path).ok())
        .map(|bytes| blake3::hash(&bytes).to_hex().to_string());
    let pricebook_hash = pricebook
        .path()
        .and_then(|path| std::fs::read(path).ok())
        .map(|bytes| blake3::hash(&bytes).to_hex().to_string());
    let started = unix_secs();
    let mut accumulated_cost = 0.0f64;

    let mut arms = vec!["baseline".to_string()];
    arms.extend(variants.iter().cloned());
    arms.dedup();
    let mut report = EvaluationReport {
        started_at_unix_secs: started,
        provider_id: provider_id.clone(),
        requested_model: requested_model.clone(),
        requested_effort: requested_effort.clone(),
        variants: arms.clone(),
        cache_conditions: ["cold", "warm"],
        optional_api_equivalent_limit_usd: limit,
        limit_note: "API-equivalent estimate, not actual subscription spend; checked after each fixture run and may overshoot by one in-flight fixture task",
        rate_source: "cached LiteLLM rate catalogue plus local user price overrides",
        cache_condition_note: "cold means a fresh Zest session; earlier fixture runs may already have warmed the provider's cache for the shared system and tool prefix, so cold/warm differences cover each task's own tail. warm_cache_read_observed and cache_rate_assumed say whether a comparison can show cache savings at all",
        rates_fetched_at_unix_secs: rates.fetched_at(),
        rates_content_blake3: rates_hash,
        pricebook_revision: pricebook.revision.clone(),
        pricebook_content_blake3: pricebook_hash,
        system_prompt_blake3: None,
        results: Vec::new(),
        unpaired_runs: Vec::new(),
        paired_analysis: Vec::new(),
        stopped_early: false,
    };
    let output_dir = workspace.join("target").join("token-efficiency");
    std::fs::create_dir_all(&output_dir).expect("create ignored evaluation output directory");
    let output = output_dir.join(format!("fixture-eval-{started}.json"));
    // Written after every task, so a later panic or interruption keeps what
    // already ran.
    let save = |report: &EvaluationReport| {
        std::fs::write(&output, serde_json::to_vec_pretty(report).unwrap())
            .expect("write ignored evaluation report");
    };
    'evaluation: for variant in &arms {
        for case in &cases {
            for cache_condition in ["cold", "warm"] {
                let temp = tempfile::Builder::new()
                    .prefix("zest-token-eval-")
                    .tempdir()
                    .expect("create disposable repository");
                reset_fixture(temp.path(), case).expect("write fixture snapshot");
                let priming = if cache_condition == "warm" {
                    Some(
                        run_fixture(temp.path(), case, variant, run_settings)
                            .await
                            .expect("run warm-cache priming task"),
                    )
                } else {
                    None
                };
                if let Some(prime) = &priming {
                    if report.system_prompt_blake3.is_none() {
                        report.system_prompt_blake3 = prime.system_prompt_blake3.clone();
                    }
                    let prime_cost = prime.breakdown.api_equivalent_usd;
                    if let Some(cost) = prime_cost {
                        accumulated_cost += cost;
                    }
                    reset_fixture(temp.path(), case).expect("restore fixture snapshot");
                    if !prime.passed {
                        let outcome = if prime.breakdown.failed_requests > 0 {
                            "provider_failure"
                        } else if prime.agent_succeeded && !prime.assertion_passed {
                            "assertion_failed"
                        } else {
                            "agent_failed"
                        };
                        report.unpaired_runs.push(UnpairedRun {
                            case_id: case.id.clone(),
                            category: case.category.clone(),
                            variant: variant.clone(),
                            phase: "warm_cache_priming",
                            outcome,
                            agent_succeeded: prime.agent_succeeded,
                            assertion_passed: prime.assertion_passed,
                            provider_rounds: prime.rounds,
                            latency_ms: prime.latency_ms,
                            tool_errors: prime.tool_errors,
                            breakdown: prime.breakdown.clone(),
                        });
                        eprintln!(
                            "warm-cache priming did not pass for {} (agent_succeeded={}, assertion_passed={}, rounds={}, tool_errors={}, failed_requests={}, usage_unavailable={}); skipping its measured warm run",
                            case.id,
                            prime.agent_succeeded,
                            prime.assertion_passed,
                            prime.rounds,
                            prime.tool_errors,
                            prime.breakdown.failed_requests,
                            prime.breakdown.usage_unavailable
                        );
                        save(&report);
                        if limit
                            .is_some_and(|limit| prime_cost.is_none() || accumulated_cost >= limit)
                        {
                            report.stopped_early = true;
                            eprintln!(
                                "evaluation limit reached or priming could not be priced; stopping"
                            );
                            save(&report);
                            break 'evaluation;
                        }
                        continue;
                    }
                    if let Some(limit) = limit {
                        if prime_cost.is_none() || accumulated_cost >= limit {
                            report.unpaired_runs.push(UnpairedRun {
                                case_id: case.id.clone(),
                                category: case.category.clone(),
                                variant: variant.clone(),
                                phase: "warm_cache_priming",
                                outcome: if prime_cost.is_none() {
                                    "unpriced_priming"
                                } else {
                                    "spend_limit_reached"
                                },
                                agent_succeeded: prime.agent_succeeded,
                                assertion_passed: prime.assertion_passed,
                                provider_rounds: prime.rounds,
                                latency_ms: prime.latency_ms,
                                tool_errors: prime.tool_errors,
                                breakdown: prime.breakdown.clone(),
                            });
                            report.stopped_early = true;
                            eprintln!("evaluation limit reached or priming could not be priced; stopping before the measured warm run");
                            break 'evaluation;
                        }
                    }
                }
                let measured = run_fixture(temp.path(), case, variant, run_settings)
                    .await
                    .expect("run fixture task");
                if report.system_prompt_blake3.is_none() {
                    report.system_prompt_blake3 = measured.system_prompt_blake3.clone();
                }
                if let Some(cost) = measured.breakdown.api_equivalent_usd {
                    accumulated_cost += cost;
                }
                let priming_cost = priming
                    .as_ref()
                    .and_then(|run| run.breakdown.api_equivalent_usd);
                let combined_cost = match (measured.breakdown.api_equivalent_usd, priming_cost) {
                    (Some(measured), Some(priming)) => Some(measured + priming),
                    (Some(measured), None) if priming.is_none() => Some(measured),
                    _ => None,
                };
                let result = TaskResult {
                    case_id: case.id.clone(),
                    category: case.category.clone(),
                    variant: variant.clone(),
                    cache_condition: cache_condition.into(),
                    passed: measured.passed && priming.as_ref().is_none_or(|run| run.passed),
                    priming_passed: priming.as_ref().map(|run| run.passed),
                    provider_rounds: measured.rounds,
                    priming_rounds: priming.as_ref().map_or(0, |run| run.rounds),
                    latency_ms: measured.latency_ms,
                    tool_errors: measured.tool_errors,
                    priming_tool_errors: priming.as_ref().map_or(0, |run| run.tool_errors),
                    api_equivalent_usd: measured.breakdown.api_equivalent_usd,
                    priming_api_equivalent_usd: priming_cost,
                    combined_api_equivalent_usd: combined_cost,
                    warm_cache_read_observed: (cache_condition == "warm")
                        .then_some(measured.breakdown.cache_read_tokens > 0),
                    breakdown: measured.breakdown,
                };
                println!(
                    "{} / {} / {}: {} rounds, {} ms, {} tool errors, {:?} API-equivalent",
                    result.variant,
                    result.cache_condition,
                    result.case_id,
                    result.provider_rounds,
                    result.latency_ms,
                    result.tool_errors,
                    result.combined_api_equivalent_usd
                );
                report.results.push(result);
                save(&report);
                if let Some(limit) = limit {
                    if combined_cost.is_none() || accumulated_cost >= limit {
                        report.stopped_early = true;
                        eprintln!("evaluation limit reached or could not be priced; stopping after current task");
                        break 'evaluation;
                    }
                }
            }
        }
    }

    report.paired_analysis = analyze_pairs(&report.results, &arms);
    save(&report);
    println!("content-free report: {}", output.display());
}

fn selected_variants() -> Vec<String> {
    let selected = std::env::var("ZEST_TOKEN_EVAL_VARIANTS").unwrap_or_else(|_| VARIANTS.join(","));
    let mut variants = Vec::new();
    for variant in selected
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        assert!(
            VARIANTS.contains(&variant),
            "unknown fixture variant `{variant}`"
        );
        if !variants.iter().any(|seen| seen == variant) {
            variants.push(variant.to_string());
        }
    }
    variants
}

fn reset_fixture(root: &Path, case: &FixtureCase) -> std::io::Result<()> {
    if root.exists() {
        for entry in std::fs::read_dir(root)? {
            let path = entry?.path();
            // A just-finished `cargo test` can hold files in `target/` open
            // for a moment on Windows; retry before giving up on the run.
            let mut attempt = 0;
            loop {
                let removed = if path.is_dir() {
                    std::fs::remove_dir_all(&path)
                } else {
                    std::fs::remove_file(&path)
                };
                match removed {
                    Ok(()) => break,
                    Err(_) if attempt < 5 => {
                        attempt += 1;
                        std::thread::sleep(std::time::Duration::from_millis(200 * attempt));
                    }
                    Err(error) => return Err(error),
                }
            }
        }
    }
    for (path, body) in &case.files {
        let target = root.join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(target, body)?;
    }
    if let Some(lines) = case.large_lines {
        let marker_line = case.large_marker_line.unwrap_or(lines);
        let marker = case.large_marker.as_deref().unwrap_or("ZEST_MARKER");
        let mut body = String::with_capacity(lines.saturating_mul(240));
        for line in 1..=lines {
            if line == marker_line {
                body.push_str(&format!(
                    "record {line:04} marker={marker} {}\n",
                    "x".repeat(210)
                ));
            } else {
                body.push_str(&format!("record {line:04} {}\n", "x".repeat(230)));
            }
        }
        std::fs::write(root.join("records.txt"), body)?;
    }
    Ok(())
}

async fn run_fixture(
    root: &Path,
    case: &FixtureCase,
    variant: &str,
    settings: FixtureRunSettings<'_>,
) -> Result<OneRun, Box<dyn std::error::Error>> {
    let mut config = settings.base_config.clone();
    config.agents.clear();
    config.mcp.clear();
    config.tools.bash.enabled = true;
    // The evaluation reads its own in-memory traces, whatever the user's
    // setting; nothing is written to their ledger.
    config.usage.task_traces = true;
    match variant {
        "baseline" => {}
        _ => return Err(format!("unknown variant: {variant}").into()),
    }
    let (mcp_server, mcp_catalog) = fixture_mcp();
    config.mcp.insert("fixture-catalog".into(), mcp_server);

    let usage = Arc::new(Mutex::new(Ledger::default()));
    let allowed_paths = case.writable.iter().cloned().collect();
    let root = std::fs::canonicalize(root)?;
    let mut builder = RuntimeBuilder::new(&root)
        .with_config(config)
        .with_provider(settings.provider_id)
        .with_system(zest_core::DEFAULT_SYSTEM)
        .with_mcp_catalog(mcp_catalog)
        .with_ledger(usage.clone())
        .with_approver(Arc::new(FixtureApprover {
            root: root.clone(),
            allowed_paths,
        }))
        .with_policy(Arc::new(Mutex::new(ApprovalPolicy::new(
            ApprovalMode::Manual,
        ))))
        .enable_external_agents(false);
    if let Some(model) = settings.model {
        builder = builder.with_model(model);
    }
    if let Some(effort) = settings.effort {
        builder = builder.with_effort(effort);
    }
    let mut session = builder.build()?;
    let system_prompt_blake3 = session.agent.system.as_ref().map(|system| {
        blake3::hash(system.cacheable.as_bytes())
            .to_hex()
            .to_string()
    });
    if let Some(note) = &case.history_note {
        let mut history = Vec::new();
        for turn in 0..16 {
            let prior = if turn == 0 {
                format!("Earlier fixture decision: {note}")
            } else {
                format!("Earlier synthetic fixture turn {turn}: retain the documented behavior and names.")
            };
            history.push(Message::user_text(prior));
            history.push(Message::assistant(vec![serde_json::json!({
                "type": "text",
                "text": "Acknowledged; I will carry this fixture context forward."
            })]));
        }
        session.agent = session.agent.with_messages(history);
    }

    let started = Instant::now();
    let mut sink = |_event: StreamEvent<'_>| {};
    let send_result = session.agent.send(&case.prompt, &mut sink).await;
    let agent_succeeded = send_result.is_ok();
    let latency_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    let final_text = session
        .agent
        .messages
        .last()
        .map(|message| {
            message
                .content
                .iter()
                .filter_map(|block| {
                    (block.get("type").and_then(Value::as_str) == Some("text"))
                        .then(|| block.get("text").and_then(Value::as_str))
                        .flatten()
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let assertion_passed = assertion_passes(&root, case, &final_text)?;
    let traces = usage
        .lock()
        .map_err(|_| "usage ledger lock poisoned")?
        .tasks()
        .to_vec();
    let family = task_family(&traces);
    let breakdown = request_breakdown(&family, settings.prices);
    let rounds = family.iter().map(|task| task.requests.len() as u64).sum();
    let tool_errors = family
        .iter()
        .flat_map(|task| &task.tools)
        .filter(|tool| tool.is_error)
        .count() as u64;
    Ok(OneRun {
        passed: agent_succeeded && assertion_passed,
        agent_succeeded,
        assertion_passed,
        rounds,
        latency_ms,
        tool_errors,
        breakdown,
        system_prompt_blake3,
    })
}

/// Whether a finished run did what its fixture asked, judged by outcome: the
/// fixture's own tests for a bug fix, the whole file for an edit, and a
/// word-bounded match for an answer. Files the task was not allowed to touch
/// must be unchanged in every case.
fn assertion_passes(
    root: &Path,
    case: &FixtureCase,
    final_text: &str,
) -> Result<bool, Box<dyn std::error::Error>> {
    let untouched = case
        .files
        .iter()
        .filter(|(path, _)| !case.writable.contains(path))
        .all(|(path, body)| {
            std::fs::read_to_string(root.join(path))
                .is_ok_and(|now| normalize_file(&now) == normalize_file(body))
        });
    if !untouched {
        return Ok(false);
    }
    let assertion = &case.assertion;
    Ok(match assertion.kind.as_str() {
        "answer_contains" => assertion
            .expected()
            .any(|value| answer_contains(final_text, value, assertion.case_sensitive)),
        "file_equals" => {
            let path = assertion.path.as_ref().ok_or("file_equals needs a path")?;
            std::fs::read_to_string(root.join(path)).is_ok_and(|body| {
                assertion
                    .expected()
                    .any(|value| normalize_file(&body) == normalize_file(value))
            })
        }
        "cargo_test_passes" => cargo_test_passes(root)?,
        other => return Err(format!("unknown fixture assertion: {other}").into()),
    })
}

/// Line endings and trailing blank space are not content.
fn normalize_file(body: &str) -> String {
    body.replace("\r\n", "\n").trim_end().to_string()
}

/// Run the fixture's own tests in a separate target directory, so the check
/// neither reuses the model's build nor leaves `target/` in the fixture.
fn cargo_test_passes(root: &Path) -> std::io::Result<bool> {
    // Model-written code runs here; an endless loop must fail the task, not
    // hang the evaluation.
    const DEADLINE: std::time::Duration = std::time::Duration::from_secs(300);
    let target = tempfile::tempdir()?;
    let mut child = std::process::Command::new("cargo")
        .args(["test", "--quiet", "--offline"])
        .current_dir(root)
        .env("CARGO_TARGET_DIR", target.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status.success());
        }
        if started.elapsed() > DEADLINE {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(false);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// The expected text as a whole word or token: `6` is not found in `64` or
/// `6.5`, nor `Rust` in `trust`, but `45` is in `45s`. Case is ignored
/// unless the fixture asks for it.
fn answer_contains(answer: &str, expected: &str, case_sensitive: bool) -> bool {
    let (answer, expected) = if case_sensitive {
        (answer.to_string(), expected.to_string())
    } else {
        (answer.to_lowercase(), expected.to_lowercase())
    };
    let Some(first) = expected.chars().next() else {
        return false;
    };
    let last = expected.chars().next_back().unwrap_or(first);
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    // A number only needs to stand apart from other digits, so a unit may
    // follow it; a word must stand apart from letters and digits alike.
    let joins = |edge: char, neighbour: char| {
        if edge.is_ascii_digit() {
            neighbour.is_ascii_digit()
        } else {
            is_word(edge) && is_word(neighbour)
        }
    };
    answer.match_indices(&expected).any(|(start, found)| {
        let before = answer[..start].chars().next_back();
        let rest = &answer[start + found.len()..];
        let mut following = rest.chars();
        let after = following.next();
        let decimal = last.is_ascii_digit()
            && after == Some('.')
            && following.next().is_some_and(|c| c.is_ascii_digit());
        !before.is_some_and(|c| joins(first, c))
            && !after.is_some_and(|c| joins(last, c))
            && !decimal
    })
}

struct FixtureApprover {
    root: PathBuf,
    allowed_paths: HashSet<String>,
}

#[async_trait]
impl Approver for FixtureApprover {
    async fn decide(&self, request: &ApprovalRequest) -> ApprovalDecision {
        if matches!(request.tool_name.as_str(), "write_file" | "edit_file")
            && self
                .allowed_paths
                .contains(&request.preview.path.replace('\\', "/"))
        {
            return ApprovalDecision::AllowOnce;
        }
        let expected = format!(
            "Run `cargo test --quiet` in `{}`",
            zest_core::display_path(&self.root)
        );
        if request.tool_name == "bash"
            && request.preview.path == "cargo test --quiet"
            && request.preview.summary == expected
        {
            return ApprovalDecision::AllowOnce;
        }
        ApprovalDecision::Deny
    }
}

fn fixture_mcp() -> (McpServerConfig, McpCatalog) {
    let server = McpServerConfig {
        command: "fixture-server-is-never-started".into(),
        args: Vec::new(),
        url: None,
        headers: BTreeMap::new(),
        header_credentials: BTreeMap::new(),
        env_vars: Vec::new(),
        enabled: true,
        timeout_secs: 5,
    };
    let tools = (0..8)
        .map(|index| McpToolDef {
            name: format!("fixture_search_{index}"),
            description: format!("Search a repository fixture collection number {index}."),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "query": { "type": "string" } },
                "required": ["query"],
                "additionalProperties": false
            }),
        })
        .collect();
    let mut catalog = McpCatalog::default();
    catalog.set("fixture-catalog", tools);
    (server, catalog)
}

fn task_family(tasks: &[zest_core::TaskUsageRecord]) -> Vec<zest_core::TaskUsageRecord> {
    let Some(root) = tasks.iter().rev().find(|task| task.kind == "turn") else {
        return Vec::new();
    };
    let mut ids = HashSet::from([root.task_id.clone()]);
    let mut correlations = HashSet::new();
    let mut included = HashSet::new();
    loop {
        let mut changed = false;
        for task in tasks {
            if included.contains(&task.task_id) {
                continue;
            }
            if task.task_id == root.task_id
                || task
                    .parent_task_id
                    .as_ref()
                    .is_some_and(|parent| ids.contains(parent))
                || task
                    .correlation_id
                    .as_ref()
                    .is_some_and(|job| correlations.contains(job))
            {
                included.insert(task.task_id.clone());
                ids.insert(task.task_id.clone());
                correlations.extend(
                    task.tools
                        .iter()
                        .filter_map(|tool| tool.correlation_id.clone()),
                );
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    tasks
        .iter()
        .filter(|task| included.contains(&task.task_id))
        .cloned()
        .collect()
}

fn request_breakdown(tasks: &[zest_core::TaskUsageRecord], prices: &Prices) -> RequestBreakdown {
    let mut result = RequestBreakdown {
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        estimated_prompt_tokens: 0,
        provider_prompt_tokens: None,
        prompt_estimate_delta_tokens: None,
        cache_tokens_not_split_by_section: 0,
        cache_rate_assumed: false,
        source_estimates: RequestSourceEstimates::default(),
        elapsed_ms: 0,
        failed_requests: 0,
        usage_unavailable: 0,
        priced_percent: 0.0,
        api_equivalent_usd: None,
    };
    let mut reported_tokens = 0u64;
    let mut priced_tokens = 0u64;
    let mut any_usage = false;
    let mut all_usage_available = true;
    let mut all_usage_priced = true;
    let mut api_equivalent_usd = 0.0;
    let mut provider_prompt_tokens = 0u64;
    for request in tasks.iter().flat_map(|task| &task.requests) {
        result.elapsed_ms = result.elapsed_ms.saturating_add(request.elapsed_ms);
        result.failed_requests += u64::from(request.failed);
        add_estimates(&mut result.source_estimates, request.source_estimates);
        if !request.usage_available {
            result.usage_unavailable += 1;
            all_usage_available = false;
            all_usage_priced = false;
            continue;
        }
        any_usage = true;
        result.input_tokens = result.input_tokens.saturating_add(request.input_tokens);
        result.output_tokens = result.output_tokens.saturating_add(request.output_tokens);
        result.cache_read_tokens = result
            .cache_read_tokens
            .saturating_add(request.cache_read_tokens);
        result.cache_write_tokens = result
            .cache_write_tokens
            .saturating_add(request.cache_write_tokens);
        provider_prompt_tokens = provider_prompt_tokens
            .saturating_add(request.input_tokens)
            .saturating_add(request.cache_read_tokens)
            .saturating_add(request.cache_write_tokens);
        let request_tokens = request
            .input_tokens
            .saturating_add(request.output_tokens)
            .saturating_add(request.cache_read_tokens)
            .saturating_add(request.cache_write_tokens);
        reported_tokens = reported_tokens.saturating_add(request_tokens);
        let counts = zest_core::pricing::Counts {
            input_tokens: request.input_tokens,
            output_tokens: request.output_tokens,
            cache_write_tokens: request.cache_write_tokens,
            cache_read_tokens: request.cache_read_tokens,
        };
        if let Some(rate) = prices.lookup(&request.provider_id, request.billed_model()) {
            result.cache_rate_assumed |= (request.cache_read_tokens > 0
                && !rate.states_cache_read())
                || (request.cache_write_tokens > 0 && !rate.states_cache_write());
        }
        if let Some(cost) = prices.price(&request.provider_id, request.billed_model(), &counts) {
            priced_tokens = priced_tokens.saturating_add(request_tokens);
            api_equivalent_usd += cost.cost_usd;
        } else {
            all_usage_priced = false;
        }
    }
    if any_usage && all_usage_priced {
        result.api_equivalent_usd = Some(api_equivalent_usd);
    }
    result.estimated_prompt_tokens = total_source_estimate(result.source_estimates);
    result.cache_tokens_not_split_by_section = result
        .cache_read_tokens
        .saturating_add(result.cache_write_tokens);
    if any_usage && all_usage_available {
        result.provider_prompt_tokens = Some(provider_prompt_tokens);
        let delta = i128::from(result.estimated_prompt_tokens) - i128::from(provider_prompt_tokens);
        result.prompt_estimate_delta_tokens =
            Some(delta.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64);
    }
    if reported_tokens > 0 {
        result.priced_percent = 100.0 * priced_tokens as f64 / reported_tokens as f64;
    }
    result
}

fn add_estimates(total: &mut RequestSourceEstimates, next: RequestSourceEstimates) {
    total.system_tokens = total.system_tokens.saturating_add(next.system_tokens);
    total.project_context_tokens = total
        .project_context_tokens
        .saturating_add(next.project_context_tokens);
    total.skill_context_tokens = total
        .skill_context_tokens
        .saturating_add(next.skill_context_tokens);
    total.tools_tokens = total.tools_tokens.saturating_add(next.tools_tokens);
    total.user_tokens = total.user_tokens.saturating_add(next.user_tokens);
    total.history_tokens = total.history_tokens.saturating_add(next.history_tokens);
    total.tool_output_tokens = total
        .tool_output_tokens
        .saturating_add(next.tool_output_tokens);
}

fn total_source_estimate(estimates: RequestSourceEstimates) -> u64 {
    estimates
        .system_tokens
        .saturating_add(estimates.project_context_tokens)
        .saturating_add(estimates.skill_context_tokens)
        .saturating_add(estimates.tools_tokens)
        .saturating_add(estimates.user_tokens)
        .saturating_add(estimates.history_tokens)
        .saturating_add(estimates.tool_output_tokens)
}

fn analyze_pairs(results: &[TaskResult], variants: &[String]) -> Vec<PairedAnalysis> {
    let mut analysis = Vec::new();
    for variant in variants
        .iter()
        .filter(|variant| variant.as_str() != "baseline")
    {
        for cache_condition in ["cold", "warm"] {
            let deltas = results
                .iter()
                .filter(|result| {
                    result.variant == *variant && result.cache_condition == cache_condition
                })
                .filter_map(|result| {
                    let baseline = results.iter().find(|candidate| {
                        candidate.variant == "baseline"
                            && candidate.case_id == result.case_id
                            && candidate.cache_condition == result.cache_condition
                    })?;
                    Some((baseline, result))
                })
                .collect::<Vec<_>>();
            let paired = deltas
                .iter()
                .filter_map(|(baseline, result)| {
                    Some((
                        result.combined_api_equivalent_usd?
                            - baseline.combined_api_equivalent_usd?,
                        baseline,
                        result,
                    ))
                })
                .collect::<Vec<_>>();
            let cost_deltas = paired
                .iter()
                .map(|(delta, _, _)| *delta)
                .collect::<Vec<_>>();
            let (mean, low, high) = mean_confidence_interval(&cost_deltas);
            let quality_regressions = deltas
                .iter()
                .filter(|(baseline, result)| baseline.passed && !result.passed)
                .count();
            let tool_error_regressions = deltas
                .iter()
                .filter(|(baseline, result)| result.tool_errors > baseline.tool_errors)
                .count();
            analysis.push(PairedAnalysis {
                variant: variant.clone(),
                cache_condition: cache_condition.into(),
                paired_tasks: cost_deltas.len(),
                quality_regressions,
                tool_error_regressions,
                mean_cost_delta_usd: mean,
                confidence_95_low_usd: low,
                confidence_95_high_usd: high,
                fixture_promotion_evidence: quality_regressions == 0
                    && tool_error_regressions == 0
                    && paired.len() == 20
                    && high.is_some_and(|upper| upper < 0.0),
            });
        }
    }
    analysis
}

fn mean_confidence_interval(values: &[f64]) -> (Option<f64>, Option<f64>, Option<f64>) {
    if values.is_empty() {
        return (None, None, None);
    }
    let n = values.len();
    let mean = values.iter().sum::<f64>() / n as f64;
    if n < 2 {
        return (Some(mean), None, None);
    }
    let variance = values
        .iter()
        .map(|value| (value - mean).powi(2))
        .sum::<f64>()
        / (n - 1) as f64;
    let critical = match n - 1 {
        1 => 12.706,
        2 => 4.303,
        3 => 3.182,
        4 => 2.776,
        5 => 2.571,
        6 => 2.447,
        7 => 2.365,
        8 => 2.306,
        9 => 2.262,
        10 => 2.228,
        11 => 2.201,
        12 => 2.179,
        13 => 2.160,
        14 => 2.145,
        15 => 2.131,
        16 => 2.120,
        17 => 2.110,
        18 => 2.101,
        19 => 2.093,
        20 => 2.086,
        21 => 2.080,
        22 => 2.074,
        23 => 2.069,
        24 => 2.064,
        25 => 2.060,
        26 => 2.056,
        27 => 2.052,
        28 => 2.048,
        29 => 2.045,
        // Cornish-Fisher expansion of the t quantile around z; within 0.001
        // of the table from 30 degrees of freedom up.
        df => {
            let z = 1.959_964f64;
            let df = df as f64;
            z + (z.powi(3) + z) / (4.0 * df)
                + (5.0 * z.powi(5) + 16.0 * z.powi(3) + 3.0 * z) / (96.0 * df * df)
        }
    };
    let margin = critical * (variance / n as f64).sqrt();
    (Some(mean), Some(mean - margin), Some(mean + margin))
}

fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod accounting_tests {
    use super::*;
    use zest_core::ModelPrice;

    fn task(provider_id: &str, usage_available: bool) -> zest_core::TaskUsageRecord {
        zest_core::TaskUsageRecord {
            task_id: "task-test".into(),
            kind: "user_turn".into(),
            status: "completed".into(),
            requests: vec![zest_core::TaskRequestUsage {
                kind: "agent_round".into(),
                provider_id: provider_id.into(),
                requested_model: "test-model".into(),
                usage_available,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn no_usage_is_unknown_not_zero_but_reported_zero_usage_is_zero() {
        assert_eq!(
            request_breakdown(&[], &Prices::default()).api_equivalent_usd,
            None
        );

        let mut prices = Prices::default();
        prices.models.insert(
            "fixture/test-model".into(),
            ModelPrice::new(1.0, 2.0, 3.0, 0.5),
        );
        assert_eq!(
            request_breakdown(&[task("fixture", true)], &prices).api_equivalent_usd,
            Some(0.0)
        );
    }

    #[test]
    fn unavailable_or_unpriced_usage_keeps_task_cost_unknown() {
        let prices = Prices::default();
        assert_eq!(
            request_breakdown(&[task("fixture", false)], &prices).api_equivalent_usd,
            None
        );
        assert_eq!(
            request_breakdown(&[task("fixture", true)], &prices).api_equivalent_usd,
            None
        );
    }

    #[test]
    fn prompt_estimate_is_reconciled_only_when_all_provider_usage_is_known() {
        let mut known = task("fixture", true);
        known.requests[0].input_tokens = 120;
        known.requests[0].cache_read_tokens = 20;
        known.requests[0].source_estimates = RequestSourceEstimates {
            system_tokens: 70,
            tools_tokens: 25,
            user_tokens: 35,
            ..Default::default()
        };
        let breakdown = request_breakdown(&[known.clone()], &Prices::default());
        assert_eq!(breakdown.estimated_prompt_tokens, 130);
        assert_eq!(breakdown.provider_prompt_tokens, Some(140));
        assert_eq!(breakdown.prompt_estimate_delta_tokens, Some(-10));
        assert_eq!(breakdown.cache_tokens_not_split_by_section, 20);

        known.requests.push(zest_core::TaskRequestUsage {
            provider_id: "fixture".into(),
            usage_available: false,
            ..Default::default()
        });
        let breakdown = request_breakdown(&[known], &Prices::default());
        assert_eq!(breakdown.provider_prompt_tokens, None);
        assert_eq!(breakdown.prompt_estimate_delta_tokens, None);
    }

    #[test]
    fn cache_priced_at_the_input_rate_is_flagged() {
        let mut cached = task("fixture", true);
        cached.requests[0].input_tokens = 10;
        cached.requests[0].cache_read_tokens = 90;

        let mut prices = Prices::default();
        prices
            .models
            .insert("fixture/test-model".into(), ModelPrice::simple(1.0, 2.0));
        let assumed = request_breakdown(&[cached.clone()], &prices);
        assert!(assumed.cache_rate_assumed);
        assert_eq!(assumed.api_equivalent_usd, Some(100.0 / 1_000_000.0));

        prices.models.insert(
            "fixture/test-model".into(),
            ModelPrice::new(1.0, 2.0, 1.25, 0.1),
        );
        let stated = request_breakdown(&[cached], &prices);
        assert!(!stated.cache_rate_assumed);
    }

    #[test]
    fn t_quantiles_past_the_table_stay_above_the_normal_value() {
        let values_with_df = |df: usize| (0..=df).map(|i| i as f64).collect::<Vec<_>>();
        for df in [30usize, 40, 60, 120] {
            let values = values_with_df(df);
            let (mean, low, _) = mean_confidence_interval(&values);
            let n = values.len() as f64;
            let sd = (values
                .iter()
                .map(|v| (v - mean.unwrap()).powi(2))
                .sum::<f64>()
                / (n - 1.0))
                .sqrt();
            let critical = (mean.unwrap() - low.unwrap()) / (sd / n.sqrt());
            let expected = match df {
                30 => 2.042,
                40 => 2.021,
                60 => 2.000,
                _ => 1.980,
            };
            assert!((critical - expected).abs() < 0.002, "df {df}: {critical}");
        }
    }

    #[test]
    fn worker_traces_join_the_turn_through_their_job_id() {
        let mut turn = task("fixture", true);
        turn.task_id = "turn".into();
        turn.kind = "turn".into();
        turn.tools.push(zest_core::TaskToolUsage {
            name: "delegate".into(),
            correlation_id: Some("job-1".into()),
            ..Default::default()
        });
        let mut worker = task("fixture", true);
        worker.task_id = "worker".into();
        worker.kind = "worker".into();
        worker.correlation_id = Some("job-1".into());
        let mut unrelated = task("fixture", true);
        unrelated.task_id = "other".into();
        unrelated.kind = "worker".into();
        unrelated.correlation_id = Some("job-2".into());

        let family = task_family(&[turn, worker, unrelated]);
        let ids: Vec<_> = family.iter().map(|task| task.task_id.as_str()).collect();
        assert_eq!(ids, ["turn", "worker"]);
    }
}
