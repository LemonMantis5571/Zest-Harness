use std::fmt::Write as _;
use std::io::Write as _;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use zest_core::{
    detect_all, ApprovalDecision, ApprovalPreview, ApprovalRequest, Approver, AuthStatus, Config,
    Ledger, Prices, ProviderCommandRequest, ProviderFileChangeRequest, ProviderInteractionHost,
    ProviderQuestionRequest, ProviderRegistry, RuntimeBuilder, StreamEvent, Thread, ThreadStore,
    ToolRisk, DEFAULT_SYSTEM,
};

mod serve;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if matches!(
        std::env::args().nth(1).as_deref(),
        Some("--help") | Some("-h")
    ) {
        print_help();
        return Ok(());
    }

    if let Err(err) = zest_core::ensure_user_config() {
        eprintln!("warning: could not create the user config: {err}");
    }
    zest_core::load_env();

    match std::env::args().nth(1).as_deref() {
        // Terminal form of the launch picker.
        Some("auth") => {
            print_auth();
            return Ok(());
        }
        Some("usage") => {
            let args: Vec<String> = std::env::args().skip(2).collect();
            if let Some(unknown) = args.iter().find(|arg| arg.as_str() != "--tasks") {
                anyhow::bail!("unknown usage option `{unknown}` (try: zest usage --tasks)");
            }
            // Refresh before printing rather than after, so the figures on
            // screen match the rates reported beneath them. At most one request
            // a day; a failure just prices against the cached copy.
            let catalog = zest_core::rates::refresh(false).await;
            print_usage(&catalog, args.iter().any(|arg| arg == "--tasks"));
            return Ok(());
        }
        Some("doctor") => {
            let args: Vec<String> = std::env::args().skip(2).collect();
            if args.iter().any(|a| matches!(a.as_str(), "--help" | "-h")) {
                print_doctor_help();
                return Ok(());
            }
            if let Some(unknown) = args.iter().find(|a| a.as_str() != "--live") {
                anyhow::bail!("unknown doctor option `{unknown}` (try: zest doctor --help)");
            }
            let live = args.iter().any(|a| a == "--live");
            if !live {
                print_doctor_help();
                std::process::exit(2);
            }
            run_doctor_live().await?;
            return Ok(());
        }
        Some("run") => {
            let args: Vec<String> = std::env::args().skip(2).collect();
            if args.iter().any(|a| matches!(a.as_str(), "--help" | "-h")) {
                print_run_help();
                return Ok(());
            }
            run_headless(args).await?;
            return Ok(());
        }
        Some("serve") => {
            let args: Vec<String> = std::env::args().skip(2).collect();
            serve::run(args).await?;
            return Ok(());
        }
        _ => {}
    }

    let root = std::env::current_dir()?;
    let effort = std::env::var("ZEST_EFFORT").unwrap_or_else(|_| "high".to_string());

    let config = Config::find(&root)?;
    for issue in config.lint() {
        eprintln!("\x1b[33mwarning:\x1b[0m {issue}");
    }

    let runtime = RuntimeBuilder::new(&root)
        .with_config(config)
        .with_effort(effort)
        .with_system(DEFAULT_SYSTEM)
        .enable_external_agents(true)
        .register_write_tools(true)
        .register_exec_tools(true)
        .with_approver(Arc::new(PromptApprover))
        .build()?;

    let mut agent = runtime.agent;
    agent.provider_interaction = Some(Arc::new(PromptApprover));

    println!(
        "zest — {} · {} · root {}",
        agent.model,
        runtime.provider_id,
        root.display()
    );
    if runtime.registry.len() > 1 {
        let others: Vec<_> = runtime
            .registry
            .ids()
            .filter(|id| *id != runtime.provider_id)
            .collect();
        println!("also configured: {}", others.join(", "));
        if !runtime.config.agents.is_empty() {
            println!("external workers: configured through ACP/headless CLI");
        }
    }
    println!("tools: {}", agent.tool_names().join(", "));
    println!("note: writes and non-read-only commands prompt here for y/N");
    println!("/btw [question] for a temporary side conversation; /back to return");
    println!("ctrl-c to quit\n");

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut side: Option<zest_core::btw::SideConversation> = None;
    loop {
        print!(
            "\x1b[1m{}>\x1b[0m ",
            if side.is_some() { "btw " } else { "" }
        );
        std::io::stdout().flush()?;

        let Some(line) = lines.next_line().await? else {
            break; // EOF
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if side.is_some() && line.eq_ignore_ascii_case("/back") {
            side = None;
            println!("Returned to the main conversation.\n");
            continue;
        }
        let question = zest_core::commands::btw_question(line);
        if question.is_some() && side.is_none() {
            side = Some(agent.side_conversation());
            println!("Temporary side conversation. /back discards it and returns.\n");
        }
        let line = question.unwrap_or(line);
        if line.is_empty() {
            continue;
        }

        let mut render = Renderer::default();
        let mut on_event = |ev: StreamEvent<'_>| render.handle(ev);

        if let Some(side) = side.as_mut() {
            if let Err(error) = side
                .send(line, &zest_core::CancelToken::new(), &mut on_event)
                .await
            {
                eprintln!("\n\x1b[31merror:\x1b[0m {error}");
            }
            println!("\n");
            continue;
        }

        if let Err(e) = agent.send(line, &mut on_event).await {
            eprintln!("\n\x1b[31merror:\x1b[0m {e}");
        }
        println!("\n");
    }

    Ok(())
}

fn print_help() {
    println!(
        "\
zest — local-first coding workbench

USAGE
  zest                         Start the interactive terminal client
  zest auth                    Show provider authentication status
  zest usage [--tasks]         Show local usage totals and recent task costs
  zest doctor --live           Run the opt-in live read-only check
  zest run --jsonl -- PROMPT   Run one deny-only JSONL/headless turn
  zest serve --project PATH [--policy trusted] [--init]

OPTIONS
  -h, --help                  Show this help

Run `zest doctor --help`, `zest run --jsonl --help`, or `zest serve --help`
for command details.
"
    );
}

fn print_run_help() {
    println!(
        "\
zest run — one deny-only headless turn

USAGE
  zest run --jsonl -- PROMPT
  echo PROMPT | zest run --jsonl

OPTIONS
  --jsonl                     Emit the zest-jsonl-v1 protocol (required)
  --json                     Compatibility alias for --jsonl
  --provider ID               Use a configured provider for this turn
  --model ID                  Use a configured model for this turn
  --effort LEVEL              Request a supported effort level
  -h, --help                 Show this help

Approvals are reported and denied instead of waiting for an interactive window.
"
    );
}

/// Run one turn as a small, line-delimited JSON protocol.
///
/// The protocol deliberately keeps approval non-interactive: a gated tool
/// emits `approval_needed` and is denied. This makes CI and editor integrations
/// deterministic while preserving the same agent/tool loop as the desktop.
async fn run_headless(args: Vec<String>) -> anyhow::Result<()> {
    let mut json = false;
    let mut model: Option<String> = None;
    let mut provider: Option<String> = None;
    let mut effort: Option<String> = None;
    let mut prompt_parts = Vec::new();

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--json" | "--jsonl" => json = true,
            "--model" => {
                index += 1;
                model = Some(
                    args.get(index)
                        .ok_or_else(|| anyhow::anyhow!("--model needs a value"))?
                        .clone(),
                );
            }
            "--provider" => {
                index += 1;
                provider = Some(
                    args.get(index)
                        .ok_or_else(|| anyhow::anyhow!("--provider needs a value"))?
                        .clone(),
                );
            }
            "--effort" => {
                index += 1;
                effort = Some(
                    args.get(index)
                        .ok_or_else(|| anyhow::anyhow!("--effort needs a value"))?
                        .clone(),
                );
            }
            "--" => {
                prompt_parts.extend(args[index + 1..].iter().cloned());
                break;
            }
            value if value.starts_with('-') => {
                anyhow::bail!("unknown run option `{value}` (try: zest run --jsonl -- PROMPT)");
            }
            value => prompt_parts.push(value.to_string()),
        }
        index += 1;
    }

    if !json {
        anyhow::bail!("headless mode requires --jsonl (legacy --json is also accepted)");
    }

    let prompt = if prompt_parts.is_empty() {
        let mut input = String::new();
        tokio::io::stdin().read_to_string(&mut input).await?;
        input.trim().to_string()
    } else {
        prompt_parts.join(" ").trim().to_string()
    };
    if prompt.is_empty() {
        anyhow::bail!("run needs a prompt argument or stdin input");
    }

    let root = std::env::current_dir()?;
    let config = Config::find(&root)?;
    for issue in config.lint() {
        eprintln!("warning: {issue}");
    }

    let mut builder = RuntimeBuilder::new(&root)
        .with_config(config)
        .with_effort(
            effort
                .or_else(|| std::env::var("ZEST_EFFORT").ok())
                .unwrap_or_else(|| "high".to_string()),
        )
        .with_system(DEFAULT_SYSTEM)
        .enable_external_agents(true)
        .register_write_tools(true)
        .register_exec_tools(true)
        .with_approver(Arc::new(JsonApprover));
    if let Some(provider) = provider {
        builder = builder.with_provider(provider);
    }
    if let Some(model) = model {
        builder = builder.with_model(model);
    }

    let runtime = builder.build()?;
    emit_json(serde_json::json!({
        "kind": "session",
        "protocol": "zest-jsonl-v1",
        "provider": runtime.provider_id,
        "model": runtime.model,
        "effort": runtime.effort,
    }));

    let mut agent = runtime.agent;
    let mut on_event = |event: StreamEvent<'_>| emit_stream_json(event);
    match agent.send(&prompt, &mut on_event).await {
        Ok(()) => emit_json(serde_json::json!({ "kind": "done" })),
        Err(err) => {
            // The Display form carries an internal tag (`stream provider:<code>:`).
            // When the provider wrote the reason for a person, quote just that.
            let message = err
                .provider_user_message()
                .map(str::to_string)
                .unwrap_or_else(|| err.to_string());
            emit_json(serde_json::json!({
                "kind": "error",
                "message": &message,
            }));
            // Exit with the same words the protocol line carried, so a caller
            // reading stderr and a caller reading JSONL agree.
            return Err(anyhow::anyhow!(message));
        }
    }

    Ok(())
}

fn emit_json(value: serde_json::Value) {
    println!("{value}");
    let _ = std::io::stdout().flush();
}

fn emit_stream_json(event: StreamEvent<'_>) {
    match event {
        StreamEvent::Text(text) if !text.is_empty() => {
            emit_json(serde_json::json!({ "kind": "text", "text": text }));
        }
        StreamEvent::Thinking(text) if !text.is_empty() => {
            emit_json(serde_json::json!({ "kind": "thinking", "text": text }));
        }
        StreamEvent::ProviderActivity { id, title, status } => emit_json(serde_json::json!({
            "kind": "provider_activity",
            "id": id,
            "title": title,
            "status": status,
        })),
        StreamEvent::ToolCallStart { name, id } => emit_json(serde_json::json!({
            "kind": "tool_call_start",
            "name": name,
            "id": id,
        })),
        StreamEvent::ToolCallUpdate { name, id, metadata } => emit_json(serde_json::json!({
            "kind": "tool_call_update",
            "name": name,
            "id": id,
            "metadata": metadata,
        })),
        StreamEvent::ToolCallResult {
            name,
            id,
            summary,
            is_error,
            path,
            diff,
            metadata,
        } => emit_json(serde_json::json!({
            "kind": "tool_call_result",
            "name": name,
            "id": id,
            "summary": summary,
            "isError": is_error,
            "path": path,
            "diff": diff,
            "metadata": metadata.and_then(|value| serde_json::to_value(value).ok()),
        })),
        StreamEvent::ApprovalNeeded {
            approval_id,
            tool_name,
            tool_call_id,
            risk,
            path,
            summary,
            diff,
        } => emit_json(serde_json::json!({
            "kind": "approval_needed",
            "approvalId": approval_id,
            "toolName": tool_name,
            "toolCallId": tool_call_id,
            "risk": serde_json::to_value(risk).unwrap_or(serde_json::Value::Null),
            "path": path,
            "summary": summary,
            "diff": diff,
        })),
        StreamEvent::QuestionNeeded {
            question_id,
            tool_call_id,
            prompt,
            choices,
            multiple,
            placeholder,
        } => emit_json(serde_json::json!({
            "kind": "question_needed",
            "questionId": question_id,
            "toolCallId": tool_call_id,
            "question": prompt,
            "choices": choices,
            "multiple": multiple,
            "placeholder": placeholder,
        })),
        StreamEvent::ModelSubstituted { requested, served } => emit_json(serde_json::json!({
            "kind": "model_substituted",
            "requested": requested,
            "served": served,
        })),
        StreamEvent::ResumeHandle(_) => {}
        StreamEvent::Text(_) | StreamEvent::Thinking(_) => {}
    }
}

struct JsonApprover;

#[async_trait::async_trait]
impl Approver for JsonApprover {
    async fn decide(&self, request: &ApprovalRequest) -> ApprovalDecision {
        // The agent emits the corresponding event before waiting here. Keep
        // this deny-only fallback as a second guard if a future tool bypasses
        // that event path.
        emit_json(serde_json::json!({
            "kind": "approval_decision",
            "approvalId": request.approval_id,
            "decision": "deny",
        }));
        ApprovalDecision::Deny
    }
}

fn print_doctor_help() {
    eprintln!(
        "\
zest doctor --live

Opt-in live acceptance checks. Spends real quota.

  --live
      One read-only tool turn against README.md. Verifies streaming, tool
      completion, usage-ledger delta, and thread persistence. Write tools and
      external workers are disabled.

Requires a working provider config (see zest.toml) and a
README.md in the workspace root.

This is manual on purpose — do not wire it into CI.
"
    );
}

/// One real Messages-API turn: read README.md, assert stream/tool/usage/persist.
async fn run_doctor_live() -> anyhow::Result<()> {
    let root = std::env::current_dir()?;
    let readme = root.join("README.md");
    if !readme.is_file() {
        anyhow::bail!("doctor --live needs README.md in {}", root.display());
    }

    println!("zest doctor --live");
    println!("workspace: {}", root.display());
    println!("note: spends quota; read-only tools only\n");

    let config = Config::find(&root)?;
    for issue in config.lint() {
        eprintln!("\x1b[33mwarning:\x1b[0m {issue}");
    }

    // Isolated ledger file so doctor does not mix with the global usage book.
    let ledger_path = root.join(".zest").join("doctor-usage.json");
    let _ = std::fs::remove_file(&ledger_path);
    let ledger = Arc::new(Mutex::new(Ledger::load_from(&ledger_path)));
    let before_requests = 0u64;

    let runtime = RuntimeBuilder::new(&root)
        .with_config(config)
        .with_system(
            "You are running zest doctor --live. Call read_file on README.md \
             (path exactly README.md), then reply with one short sentence that \
             includes the word zest. Do not write files or call other tools.",
        )
        .with_ledger(ledger.clone())
        .enable_external_agents(false)
        .register_write_tools(false)
        .register_exec_tools(false)
        .build()?;

    println!(
        "provider {} · model {} · effort {}",
        runtime.provider_id, runtime.model, runtime.effort
    );

    let mut agent = runtime.agent;
    let mut saw_text = false;
    let mut saw_tool_start = false;
    let mut saw_tool_ok = false;
    let mut tool_error: Option<String> = None;

    let mut on_event = |ev: StreamEvent<'_>| match ev {
        StreamEvent::Text(t) => {
            if !t.is_empty() {
                saw_text = true;
                print!("{t}");
                let _ = std::io::stdout().flush();
            }
        }
        StreamEvent::Thinking(t) => {
            if !t.is_empty() {
                print!("\x1b[90m{t}\x1b[0m");
                let _ = std::io::stdout().flush();
            }
        }
        StreamEvent::ProviderActivity { title, status, .. } => {
            let marker = match status {
                "running" | "in_progress" => "→",
                "done" | "completed" | "complete" => "✓",
                _ => "✕",
            };
            println!("\n{marker} {title}");
        }
        StreamEvent::ToolCallStart { name, .. } => {
            println!("\n→ {name}");
            if name == "read_file" {
                saw_tool_start = true;
            }
        }
        StreamEvent::ToolCallUpdate { .. } => {}
        StreamEvent::ToolCallResult {
            name,
            summary,
            is_error,
            ..
        } => {
            if is_error {
                println!("✗ {name} {summary}");
                tool_error = Some(format!("{name}: {summary}"));
            } else {
                println!("✓ {name}");
                if name == "read_file" {
                    saw_tool_ok = true;
                }
            }
        }
        StreamEvent::ApprovalNeeded { tool_name, .. } => {
            tool_error = Some(format!("unexpected approval for {tool_name}"));
        }
        StreamEvent::QuestionNeeded { .. } => {
            tool_error = Some("unexpected interactive question".into());
        }
        StreamEvent::ModelSubstituted { served, .. } => {
            println!("\n\x1b[33m! The selected model was unavailable; this response used {served} instead.\x1b[0m");
        }
        StreamEvent::ResumeHandle(_) => {}
    };

    agent
        .send(
            "Read README.md with the read_file tool, then confirm briefly.",
            &mut on_event,
        )
        .await?;
    println!("\n");

    if let Some(err) = tool_error {
        anyhow::bail!("doctor tool failure: {err}");
    }
    if !saw_tool_start {
        anyhow::bail!("doctor failed: model never started read_file");
    }
    if !saw_tool_ok {
        anyhow::bail!("doctor failed: read_file did not complete successfully");
    }
    if !saw_text {
        anyhow::bail!("doctor failed: no streamed text deltas");
    }

    let provider_id = agent.provider_id().to_string();
    // Reload from disk so success reflects durable metering, not just RAM.
    let after = {
        let mut guard = ledger.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        guard.reload_from_disk();
        guard
            .get(&provider_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("doctor failed: no ledger entry for `{provider_id}`"))?
    };
    if after.requests <= before_requests {
        anyhow::bail!(
            "doctor failed: usage did not increase (before={before_requests}, after={})",
            after.requests
        );
    }

    // Persist + restore the wire history the way a session reopen would.
    let store = ThreadStore::open(&root)?;
    let mut thread = Thread::new().with_provider(&provider_id);
    thread.title = Some("doctor --live".into());
    thread.agent_messages = agent.messages.clone();
    store.save(&thread)?;
    let loaded = store.load_with_recovery(&thread.id)?;
    if loaded.thread.agent_messages.len() < 2 {
        anyhow::bail!("doctor failed: persisted thread missing wire history");
    }
    if loaded.thread.provider_id.as_deref() != Some(provider_id.as_str()) {
        anyhow::bail!("doctor failed: provider_id not restored");
    }

    println!("checks:");
    println!("  streaming text ........ ok");
    println!("  read_file tool ........ ok");
    println!(
        "  usage delta ........... ok ({} → {} req on {provider_id})",
        before_requests, after.requests
    );
    println!("  persistence ........... ok (thread {})", loaded.thread.id);
    println!("\n\x1b[32mdoctor --live passed\x1b[0m");
    Ok(())
}

fn print_auth() {
    println!("\n\x1b[1mProviders\x1b[0m\n");

    let mut seen = std::collections::HashSet::new();
    for slot in detect_all() {
        seen.insert(slot.id.to_string());
        print_auth_row(slot.label, slot.method, &slot.status);
    }

    // `detect_all` is the launch-picker catalogue. Configured OpenAI-compatible
    // parents such as DeepSeek only appear when zest.toml is consulted, which
    // is how `zest auth` missed a working `DEEPSEEK_API_KEY`.
    let root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    if let Ok(config) = Config::find(&root) {
        let (registry, skipped) = ProviderRegistry::from_config_at(&config, &root);
        for id in config.providers.keys() {
            if !seen.insert(id.clone()) {
                continue;
            }
            let method = config
                .providers
                .get(id)
                .map(provider_kind_method)
                .unwrap_or("API key");
            let status = registry
                .get(id)
                .map(|provider| provider.auth_status())
                .or_else(|| {
                    skipped.iter().find(|row| row.id == *id).map(|row| {
                        if row.reason.contains("not set") {
                            AuthStatus::Unconfigured
                        } else {
                            AuthStatus::Unknown {
                                reason: row.reason.clone(),
                            }
                        }
                    })
                })
                .unwrap_or(AuthStatus::Unconfigured);
            print_auth_row(id, method, &status);
        }
    }

    println!("\n\x1b[90m● selectable   ○ unavailable\x1b[0m\n");
}

fn provider_kind_method(config: &zest_core::ProviderConfig) -> &'static str {
    match config {
        zest_core::ProviderConfig::Anthropic { .. } => "API key",
        zest_core::ProviderConfig::OpenaiCompatible { .. } => "API key",
        zest_core::ProviderConfig::ClaudeCode { .. } => "Claude sign-in",
        zest_core::ProviderConfig::CodexCli { .. } => "Codex CLI",
        zest_core::ProviderConfig::CodexOAuth { .. } => "ChatGPT sign-in",
        zest_core::ProviderConfig::CursorAcp { .. } => "Cursor subscription",
    }
}

fn print_auth_row(label: &str, method: &str, status: &AuthStatus) {
    let (mark, detail) = match status {
        AuthStatus::Ready { account } => (
            "\x1b[32m●\x1b[0m",
            account.clone().unwrap_or_else(|| "signed in".into()),
        ),
        // Deliberately not red: we cannot see the credentials, which is not
        // the same as their being absent.
        AuthStatus::Unknown { reason } => ("\x1b[33m●\x1b[0m", reason.clone()),
        AuthStatus::NotLoggedIn { fix } => ("\x1b[90m○\x1b[0m", format!("run: {fix}")),
        AuthStatus::Unconfigured => ("\x1b[90m○\x1b[0m", "no key set".into()),
    };

    println!(
        "  {mark} \x1b[1m{:<13}\x1b[0m \x1b[90m{:<20}\x1b[0m {detail}",
        label, method
    );
}

/// Spend and headroom are printed as separate lines on purpose. They answer
/// different questions and one of them is not ours to measure.
fn print_usage(catalog: &zest_core::RateCatalog, show_tasks: bool) {
    let ledger = Ledger::load();

    println!("\n\x1b[1mUsage\x1b[0m");
    if let Some(path) = ledger.path() {
        println!("\x1b[90m{}\x1b[0m", path.display());
    }
    println!();

    if ledger.is_empty() && ledger.tasks().is_empty() {
        println!("  \x1b[90mNothing recorded yet.\x1b[0m\n");
        return;
    }

    for (id, usage) in ledger.entries() {
        println!("  \x1b[1m{id}\x1b[0m");
        println!(
            "    spent      {} req · {} in · {} out  \x1b[90m(measured by Zest)\x1b[0m",
            usage.requests,
            compact(usage.input_tokens),
            compact(usage.output_tokens),
        );
        match &usage.headroom {
            Some(h) if !h.is_empty() => {
                let req = h
                    .requests_remaining
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "?".into());
                println!(
                    "    headroom   {req} req remaining  \x1b[90m(provider-reported throughput)\x1b[0m"
                );
            }
            _ => {
                println!("    headroom   \x1b[90mnot reported by provider\x1b[0m");
            }
        }
        println!();
    }

    print_recent_cost(&ledger, catalog);
    if show_tasks {
        let traces_on = std::env::current_dir()
            .ok()
            .and_then(|dir| zest_core::Config::find(dir).ok())
            .is_some_and(|config| config.usage.task_traces);
        if traces_on {
            print_task_costs(&ledger, &Prices::load().with_catalog(catalog.clone()));
        } else {
            println!("\n  \x1b[1mrecent tasks\x1b[0m");
            println!(
                "    \x1b[90mTask traces are off. Add `[usage]` with `task_traces = true` to zest.toml to keep content-free traces on this machine for 30 days.\x1b[0m"
            );
        }
    }
}

fn print_task_costs(ledger: &Ledger, prices: &Prices) {
    print!("{}", render_task_costs(ledger, prices));
}

fn render_task_costs(ledger: &Ledger, prices: &Prices) -> String {
    let mut output = String::new();
    writeln!(output, "\n  \x1b[1mrecent tasks (last 30 days)\x1b[0m").unwrap();
    if ledger.tasks().is_empty() {
        writeln!(output, "    \x1b[90mNo task traces recorded yet.\x1b[0m").unwrap();
        return output;
    }
    for task in ledger.tasks().iter().rev().take(20) {
        let mut counts = zest_core::TokenCounts::default();
        let mut cost = 0.0;
        let mut unpriced = false;
        let mut unknown_usage = 0usize;
        let mut requests = 0u64;
        let mut latency_ms = 0u64;
        let mut failed_tools = 0usize;
        let mut failed_requests = 0usize;
        let mut reported_tokens = 0u64;
        let mut priced_tokens = 0u64;
        let mut provider_prompt_tokens = 0u64;
        let mut all_usage_available = true;
        let mut cache_tokens = 0u64;
        let mut source = zest_core::RequestSourceEstimates::default();
        for request in &task.requests {
            requests += 1;
            latency_ms = latency_ms.saturating_add(request.elapsed_ms);
            failed_requests += usize::from(request.failed);
            source.system_tokens = source
                .system_tokens
                .saturating_add(request.source_estimates.system_tokens);
            source.project_context_tokens = source
                .project_context_tokens
                .saturating_add(request.source_estimates.project_context_tokens);
            source.skill_context_tokens = source
                .skill_context_tokens
                .saturating_add(request.source_estimates.skill_context_tokens);
            source.tools_tokens = source
                .tools_tokens
                .saturating_add(request.source_estimates.tools_tokens);
            source.user_tokens = source
                .user_tokens
                .saturating_add(request.source_estimates.user_tokens);
            source.history_tokens = source
                .history_tokens
                .saturating_add(request.source_estimates.history_tokens);
            source.tool_output_tokens = source
                .tool_output_tokens
                .saturating_add(request.source_estimates.tool_output_tokens);
            if !request.usage_available {
                unknown_usage += 1;
                all_usage_available = false;
                continue;
            }
            let request_tokens = request
                .input_tokens
                .saturating_add(request.output_tokens)
                .saturating_add(request.cache_write_tokens)
                .saturating_add(request.cache_read_tokens);
            reported_tokens = reported_tokens.saturating_add(request_tokens);
            provider_prompt_tokens = provider_prompt_tokens
                .saturating_add(request.input_tokens)
                .saturating_add(request.cache_read_tokens)
                .saturating_add(request.cache_write_tokens);
            cache_tokens = cache_tokens
                .saturating_add(request.cache_write_tokens)
                .saturating_add(request.cache_read_tokens);
            counts.input_tokens = counts.input_tokens.saturating_add(request.input_tokens);
            counts.output_tokens = counts.output_tokens.saturating_add(request.output_tokens);
            counts.cache_write_tokens = counts
                .cache_write_tokens
                .saturating_add(request.cache_write_tokens);
            counts.cache_read_tokens = counts
                .cache_read_tokens
                .saturating_add(request.cache_read_tokens);
            let pricing = zest_core::pricing::Counts {
                input_tokens: request.input_tokens,
                output_tokens: request.output_tokens,
                cache_write_tokens: request.cache_write_tokens,
                cache_read_tokens: request.cache_read_tokens,
            };
            if request_tokens == 0 {
                // Nothing to price; an unknown rate cannot change a zero.
                continue;
            }
            match prices.price(&request.provider_id, request.billed_model(), &pricing) {
                Some(estimate) => {
                    cost += estimate.cost_usd;
                    priced_tokens = priced_tokens.saturating_add(request_tokens);
                }
                None => unpriced = true,
            }
        }
        failed_tools += task.tools.iter().filter(|tool| tool.is_error).count();
        let cost_text = if requests == 0 {
            "no provider request".to_string()
        } else {
            // Unpriced (tokens known, rate not) and unknown (usage never
            // reported) are different gaps, and neither is zero.
            let mut parts = Vec::new();
            if priced_tokens > 0 || (!unpriced && unknown_usage == 0) {
                parts.push(format!("~${cost:.4}"));
            }
            if unpriced {
                parts.push("unpriced tokens".to_string());
            }
            if unknown_usage > 0 {
                parts.push(format!("{unknown_usage} request(s) with unknown usage"));
            }
            parts.join(" + ")
        };
        let kind = match &task.correlation_id {
            Some(job) => format!("{} for {job}", task.kind),
            None => task.kind.clone(),
        };
        writeln!(output,
            "    {} · {} · {} req · {} provider tokens · {} ms · {} request error(s) · {} tool error(s) · {} (API-equivalent estimate)",
            kind,
            task.status,
            requests,
            compact(counts.total_tokens()),
            latency_ms,
            failed_requests,
            failed_tools,
            cost_text
        ).unwrap();
        if reported_tokens > 0 {
            let coverage = 100.0 * priced_tokens as f64 / reported_tokens as f64;
            writeln!(output,
                "      pricing coverage: {coverage:.0}% ({} / {} provider-reported tokens); subscription spend is not recorded here",
                compact(priced_tokens),
                compact(reported_tokens)
            ).unwrap();
        } else if requests > 0 && !all_usage_available {
            writeln!(
                output,
                "      provider usage unavailable; no API-equivalent cost is inferred"
            )
            .unwrap();
        } else if requests > 0 {
            writeln!(
                output,
                "      provider reported zero tokens; API-equivalent estimate is $0.0000"
            )
            .unwrap();
        }
        if task.dropped_requests > 0 || task.dropped_tools > 0 {
            writeln!(
                output,
                "      {} earlier round(s) and {} tool record(s) were dropped from this trace; the totals above are partial",
                task.dropped_requests, task.dropped_tools
            )
            .unwrap();
        }
        if requests > 0 {
            let estimated_prompt_tokens = source
                .system_tokens
                .saturating_add(source.project_context_tokens)
                .saturating_add(source.skill_context_tokens)
                .saturating_add(source.tools_tokens)
                .saturating_add(source.user_tokens)
                .saturating_add(source.history_tokens)
                .saturating_add(source.tool_output_tokens);
            if all_usage_available {
                let delta =
                    i128::from(estimated_prompt_tokens) - i128::from(provider_prompt_tokens);
                writeln!(output,
                    "      prompt estimate: {} vs {} provider-reported prompt tokens (estimate − reported: {delta:+})",
                    compact(estimated_prompt_tokens),
                    compact(provider_prompt_tokens)
                ).unwrap();
            } else {
                writeln!(output,
                    "      prompt estimate: {}; provider prompt totals are incomplete, so no reconciliation is shown",
                    compact(estimated_prompt_tokens)
                ).unwrap();
            }
            writeln!(output,
                "      estimated prompt sections (tokens): system {}, project {}, skills {}, tools {}, user {}, history {}, tool output {}; {} of the provider-reported prompt tokens were cache reads or writes, which cannot be split by section",
                compact(source.system_tokens),
                compact(source.project_context_tokens),
                compact(source.skill_context_tokens),
                compact(source.tools_tokens),
                compact(source.user_tokens),
                compact(source.history_tokens),
                compact(source.tool_output_tokens),
                compact(cache_tokens)
            ).unwrap();
        }
    }
    writeln!(output, "    \x1b[90mNo prompts, tool bodies, or project paths are stored in task traces; estimates are not bills.\x1b[0m").unwrap();
    output
}

/// The last 30 days at list rates, with its own coverage stated underneath.
///
/// The coverage line is not optional decoration. A dollar figure derived from
/// half the tokens looks exactly like one derived from all of them, and this is
/// the only thing that tells them apart.
fn print_recent_cost(ledger: &Ledger, catalog: &zest_core::RateCatalog) {
    let scan = zest_core::transcripts::scan(30);
    let report = ledger.report(
        30,
        &Prices::load().with_catalog(catalog.clone()),
        Some(&scan),
    );
    if report.totals.processed_tokens == 0 {
        return;
    }

    println!("  \x1b[1mlast 30 days\x1b[0m");
    println!(
        "    tokens     {} over {} active day{}",
        compact(report.totals.processed_tokens),
        report.totals.active_days,
        if report.totals.active_days == 1 {
            ""
        } else {
            "s"
        },
    );
    // Three shares that add to the whole prompt, not one hit rate: a lone rate
    // scores cache writes as failures, so a session busy filling its cache
    // reads the same as one whose cache never worked.
    println!(
        "    prompt     {:.0}% from cache · {:.0}% cached for later · {:.0}% read fresh",
        report.totals.served_from_cache_percent,
        report.totals.written_to_cache_percent,
        report.totals.read_fresh_percent,
    );
    if let Some(zest) = report.totals.zest {
        if (zest.served_from_cache_percent - report.totals.served_from_cache_percent).abs() >= 0.5 {
            println!(
                "    zest       {:.0}% from cache · {:.0}% cached for later · {:.0}% read fresh",
                zest.served_from_cache_percent,
                zest.written_to_cache_percent,
                zest.read_fresh_percent,
            );
        }
    }
    println!(
        "    cache      {} read{}  \x1b[90m(saved ~${:.2} at list rates)\x1b[0m",
        compact(report.totals.cached_input_tokens),
        match report.totals.cache_reuse_ratio {
            // Below ~0.3 reads per write the 1.25x write premium never comes
            // back, so caching is a net cost rather than a saving.
            Some(ratio) if ratio < 0.3 => " · costing more than it saves".to_string(),
            Some(ratio) => format!(" · each cached token reused {ratio:.1}x"),
            // No writes reported is not the same as a cold cache: OpenAI and
            // Codex cache the prefix themselves and report reads only.
            None if report.totals.cached_input_tokens > 0 => {
                " · cached by the provider, writes not reported".to_string()
            }
            None => " · nothing cached yet".to_string(),
        },
        report.totals.cache_savings_usd,
    );
    println!(
        "    cost       \x1b[1m${:.2}\x1b[0m  \x1b[90m(provider-reported + list-rate estimate, not a bill)\x1b[0m",
        report.totals.cost_usd,
    );
    println!(
        "    coverage   {:.0}% of tokens costed\x1b[90m{}{}{}{}\x1b[0m",
        report.quality.provider_reported_percent + report.quality.priced_percent,
        if report.quality.provider_reported_percent > 0.0 {
            format!(
                ", {:.0}% reported",
                report.quality.provider_reported_percent
            )
        } else {
            String::new()
        },
        if report.quality.priced_percent > 0.0 {
            format!(", {:.0}% list-priced", report.quality.priced_percent)
        } else {
            String::new()
        },
        if report.quality.unpriced_percent > 0.0 {
            format!(", {:.0}% unpriced", report.quality.unpriced_percent)
        } else {
            String::new()
        },
        if report.quality.unattributed_percent > 0.0 {
            format!(
                ", {:.0}% recorded before per-model metering",
                report.quality.unattributed_percent
            )
        } else {
            String::new()
        },
    );

    if !report.quality.unpriced_models.is_empty() {
        println!(
            "    \x1b[90mno rate for: {}\x1b[0m",
            report.quality.unpriced_models.join(", ")
        );
        if let Some(path) = &report.prices_path {
            println!("    \x1b[90madd rates in {path}\x1b[0m");
        }
    }
    println!(
        "    rates      \x1b[90m{} models{}\x1b[0m",
        report.rates.catalog_models,
        match report.rates.fetched_at {
            Some(_) if report.rates.stale => ", cached copy is due a refresh".to_string(),
            Some(_) => String::new(),
            None => ", never fetched".to_string(),
        },
    );
    println!(
        "    scanned    \x1b[90m{} CLI transcripts ({} parsed, {} unchanged) · {} turns, {} repeats dropped\x1b[0m",
        report.scan.files_scanned + report.scan.files_cached,
        report.scan.files_scanned,
        report.scan.files_cached,
        report.scan.records,
        report.scan.duplicates_dropped,
    );

    println!("\n  \x1b[1mby model\x1b[0m");
    for row in report.models.iter().take(8) {
        println!(
            "    {:<26} {:>10}  {:>8}  \x1b[90m{}\x1b[0m",
            format!("{}/{}", row.provider_id, row.model_id),
            match row.cost_usd {
                Some(cost) => format!("${cost:.2}"),
                None => "no rate".to_string(),
            },
            compact(row.tokens),
            match row.cost_source {
                zest_core::CostSource::ProviderReported => "reported",
                zest_core::CostSource::ModelPriced => "priced",
                zest_core::CostSource::Mixed => "mixed",
                zest_core::CostSource::Unpriced => "unpriced",
            },
        );
    }
    println!();
}

fn compact(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// Terminal approval gate shared by Zest-owned tools and provider-owned tools.
///
/// Anything that is not an explicit yes is a no, including EOF — a piped or
/// detached stdin must not be able to approve a write or a shell command by
/// falling off the end of input.
struct PromptApprover;

#[async_trait::async_trait]
impl Approver for PromptApprover {
    async fn decide(&self, request: &ApprovalRequest) -> ApprovalDecision {
        prompt_approval(request).await
    }
}

#[async_trait::async_trait]
impl ProviderInteractionHost for PromptApprover {
    async fn decide_command(&self, request: ProviderCommandRequest) -> ApprovalDecision {
        let approval = ApprovalRequest {
            approval_id: request.approval_id.clone(),
            tool_name: "provider_command".into(),
            tool_call_id: request.approval_id,
            risk: ToolRisk::Exec,
            preview: ApprovalPreview {
                path: request.cwd.unwrap_or_default(),
                summary: request.command,
                diff: request
                    .reason
                    .map(|reason| format!("Reason: {reason}"))
                    .unwrap_or_default(),
            },
        };
        prompt_approval(&approval).await
    }

    async fn approve_command(&self, request: ProviderCommandRequest) -> bool {
        matches!(
            self.decide_command(request).await,
            ApprovalDecision::AllowOnce | ApprovalDecision::AllowSession
        )
    }

    async fn decide_file_change(&self, request: ProviderFileChangeRequest) -> ApprovalDecision {
        let approval = ApprovalRequest {
            approval_id: request.approval_id.clone(),
            tool_name: "provider_file_change".into(),
            tool_call_id: request.approval_id,
            risk: ToolRisk::Write,
            preview: ApprovalPreview {
                path: request.path.unwrap_or_default(),
                summary: request
                    .reason
                    .unwrap_or_else(|| "Provider requested a file change".into()),
                diff: request.diff.unwrap_or_default(),
            },
        };
        prompt_approval(&approval).await
    }

    async fn approve_file_change(&self, request: ProviderFileChangeRequest) -> bool {
        matches!(
            self.decide_file_change(request).await,
            ApprovalDecision::AllowOnce | ApprovalDecision::AllowSession
        )
    }

    async fn answer_question(&self, request: ProviderQuestionRequest) -> Option<Vec<String>> {
        if !request.choices.is_empty() {
            println!("  choices:");
            for (index, choice) in request.choices.iter().enumerate() {
                println!("    {}. {choice}", index + 1);
            }
        }
        print!("  answer{}: ", if request.multiple { "s" } else { "" });
        let _ = std::io::stdout().flush();
        let line = read_prompt_line().await;
        parse_question_answers(&line, &request)
    }
}

async fn prompt_approval(request: &ApprovalRequest) -> ApprovalDecision {
    let ApprovalRequest {
        tool_name,
        risk,
        preview,
        ..
    } = request;

    println!("\n\x1b[33m? {tool_name}\x1b[0m {}", preview.summary);
    if !preview.path.trim().is_empty() {
        println!("\x1b[90m{}\x1b[0m", preview.path);
    }
    if !preview.diff.trim().is_empty() {
        // Diffs can be long; the preview is already bounded by the tool.
        println!("\x1b[90m{}\x1b[0m", preview.diff.trim_end());
    }
    print!("  allow this {}? [y/N] ", risk_word(*risk));
    let _ = std::io::stdout().flush();

    match read_prompt_line()
        .await
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "y" | "yes" => ApprovalDecision::AllowOnce,
        _ => ApprovalDecision::Deny,
    }
}

async fn read_prompt_line() -> String {
    tokio::task::spawn_blocking(|| {
        let mut buf = String::new();
        std::io::stdin().read_line(&mut buf).map(|_| buf)
    })
    .await
    .ok()
    .and_then(Result::ok)
    .unwrap_or_default()
}

fn parse_question_answers(input: &str, request: &ProviderQuestionRequest) -> Option<Vec<String>> {
    let raw_answers: Vec<_> = if request.multiple {
        input
            .split(',')
            .map(str::trim)
            .filter(|answer| !answer.is_empty())
            .collect()
    } else {
        let answer = input.trim();
        if answer.is_empty() {
            Vec::new()
        } else {
            vec![answer]
        }
    };
    if raw_answers.is_empty() || (!request.multiple && raw_answers.len() != 1) {
        return None;
    }

    raw_answers
        .into_iter()
        .map(|answer| {
            if request.choices.is_empty() {
                return Some(answer.to_string());
            }
            if let Ok(number) = answer.parse::<usize>() {
                return request.choices.get(number.checked_sub(1)?).cloned();
            }
            request
                .choices
                .iter()
                .find(|choice| choice.eq_ignore_ascii_case(answer))
                .cloned()
        })
        .collect()
}

fn risk_word(risk: ToolRisk) -> &'static str {
    match risk {
        ToolRisk::Exec => "command",
        ToolRisk::Write => "write",
        ToolRisk::Sensitive => "sensitive read",
        ToolRisk::Read => "call",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zest_core::{RequestSourceEstimates, TaskRequestUsage};

    fn question(choices: Vec<&str>, multiple: bool) -> ProviderQuestionRequest {
        ProviderQuestionRequest {
            id: "q1".into(),
            prompt: "Pick one".into(),
            choices: choices.into_iter().map(str::to_string).collect(),
            multiple,
        }
    }

    #[test]
    fn provider_question_accepts_a_numbered_choice() {
        let request = question(vec!["Rust", "Go"], false);
        assert_eq!(
            parse_question_answers("2", &request),
            Some(vec!["Go".to_string()])
        );
    }

    #[test]
    fn provider_question_accepts_multiple_named_choices() {
        let request = question(vec!["Rust", "Go", "Python"], true);
        assert_eq!(
            parse_question_answers("rust, 3", &request),
            Some(vec!["Rust".to_string(), "Python".to_string()])
        );
    }

    #[test]
    fn provider_question_rejects_an_unknown_choice() {
        let request = question(vec!["Rust", "Go"], false);
        assert_eq!(parse_question_answers("Ruby", &request), None);
    }

    #[test]
    fn task_usage_view_renders_estimates_coverage_and_content_free_activity() {
        let mut ledger = Ledger::default();
        ledger.begin_task("task-view", "turn", None);
        ledger.record_task_request(
            "task-view",
            TaskRequestUsage {
                kind: "agent".into(),
                provider_id: "unknown-provider".into(),
                requested_model: "unpriced-model".into(),
                served_model: Some("unpriced-model".into()),
                elapsed_ms: 25,
                usage_available: true,
                failed: false,
                input_tokens: 100,
                output_tokens: 20,
                cache_write_tokens: 5,
                cache_read_tokens: 10,
                source_estimates: RequestSourceEstimates {
                    system_tokens: 2,
                    project_context_tokens: 1,
                    skill_context_tokens: 1,
                    tools_tokens: 2,
                    user_tokens: 3,
                    history_tokens: 4,
                    tool_output_tokens: 1,
                },
            },
        );
        ledger.record_task_tool("task-view", "read_file", 512, false, None);
        ledger.finish_task("task-view", "completed");

        let rendered = render_task_costs(&ledger, &Prices::default());

        assert!(rendered.contains("recent tasks (last 30 days)"));
        assert!(rendered.contains("turn · completed · 1 req · 135 provider tokens · 25 ms"));
        assert!(rendered.contains("· unpriced tokens (API-equivalent estimate)"));
        assert!(rendered.contains("pricing coverage: 0% (0 / 135 provider-reported tokens)"));
        assert!(rendered.contains("prompt estimate: 14 vs 115 provider-reported prompt tokens"));
        assert!(rendered.contains("system 2, project 1, skills 1, tools 2, user 3, history 4, tool output 1; 15 of the provider-reported prompt tokens were cache reads or writes"));
        assert!(rendered.contains("No prompts, tool bodies, or project paths are stored"));
    }

    fn priced_request(requested: &str, served: Option<&str>, usage: bool) -> TaskRequestUsage {
        TaskRequestUsage {
            kind: "agent".into(),
            provider_id: "anthropic".into(),
            requested_model: requested.into(),
            served_model: served.map(Into::into),
            usage_available: usage,
            failed: !usage,
            input_tokens: if usage { 1_000_000 } else { 0 },
            ..TaskRequestUsage::default()
        }
    }

    #[test]
    fn task_usage_view_prices_like_the_ledger_and_names_each_gap() {
        let mut prices = Prices::default();
        prices.models.insert(
            "claude-opus-5".into(),
            zest_core::pricing::ModelPrice::simple(2.0, 10.0),
        );

        let mut ledger = Ledger::default();
        // A dated build of the requested alias bills as the alias.
        ledger.begin_task("dated", "turn", None);
        ledger.record_task_request(
            "dated",
            priced_request("claude-opus-5", Some("claude-opus-5-20260901"), true),
        );
        ledger.finish_task("dated", "completed");
        let rendered = render_task_costs(&ledger, &prices);
        assert!(
            rendered.contains("· ~$2.0000 (API-equivalent estimate)"),
            "{rendered}"
        );
        assert!(rendered.contains("pricing coverage: 100%"), "{rendered}");

        // A failed round is unknown usage, not an unpriced model.
        let mut ledger = Ledger::default();
        ledger.begin_correlated_task("mixed", "worker", None, Some("job-9".into()));
        ledger.record_task_request("mixed", priced_request("claude-opus-5", None, true));
        ledger.record_task_request("mixed", priced_request("claude-opus-5", None, false));
        ledger.finish_task("mixed", "completed");
        let rendered = render_task_costs(&ledger, &prices);
        assert!(
            rendered.contains("worker for job-9 · completed"),
            "{rendered}"
        );
        assert!(
            rendered.contains("~$2.0000 + 1 request(s) with unknown usage (API-equivalent"),
            "{rendered}"
        );
        assert!(!rendered.contains("unpriced"), "{rendered}");

        // Zero reported tokens on an unpriced model are still zero.
        let mut ledger = Ledger::default();
        ledger.begin_task("zero", "turn", None);
        let mut zero = priced_request("no-rate-model", None, true);
        zero.input_tokens = 0;
        ledger.record_task_request("zero", zero);
        ledger.finish_task("zero", "completed");
        let rendered = render_task_costs(&ledger, &prices);
        assert!(
            rendered.contains("· ~$0.0000 (API-equivalent"),
            "{rendered}"
        );
        assert!(
            rendered.contains("provider reported zero tokens"),
            "{rendered}"
        );
    }

    #[test]
    fn task_usage_view_keeps_missing_provider_usage_unknown() {
        let mut ledger = Ledger::default();
        ledger.begin_task("unknown-usage", "turn", None);
        ledger.record_task_request(
            "unknown-usage",
            TaskRequestUsage {
                kind: "agent".into(),
                provider_id: "provider".into(),
                requested_model: "model".into(),
                served_model: None,
                elapsed_ms: 0,
                usage_available: false,
                failed: true,
                input_tokens: 0,
                output_tokens: 0,
                cache_write_tokens: 0,
                cache_read_tokens: 0,
                source_estimates: RequestSourceEstimates {
                    user_tokens: 7,
                    ..RequestSourceEstimates::default()
                },
            },
        );
        ledger.finish_task("unknown-usage", "failed");

        let rendered = render_task_costs(&ledger, &Prices::default());

        assert!(rendered.contains("provider usage unavailable; no API-equivalent cost is inferred"));
        assert!(rendered.contains("· 1 request(s) with unknown usage (API-equivalent"));
        assert!(rendered.contains("prompt estimate: 7; provider prompt totals are incomplete"));
        assert!(!rendered.contains("provider reported zero tokens"));
    }
}

#[derive(Default)]
struct Renderer {
    thinking_open: bool,
    text_started: bool,
}

impl Renderer {
    fn handle(&mut self, ev: StreamEvent<'_>) {
        match ev {
            StreamEvent::Thinking(t) => {
                if !self.thinking_open {
                    print!("\x1b[90m");
                    self.thinking_open = true;
                }
                print!("{t}");
                let _ = std::io::stdout().flush();
            }
            StreamEvent::ProviderActivity { title, status, .. } => {
                if self.thinking_open {
                    println!("\x1b[0m");
                    self.thinking_open = false;
                }
                let marker = match status {
                    "running" | "in_progress" => "→",
                    "done" | "completed" | "complete" => "✓",
                    _ => "✕",
                };
                println!("\n\x1b[90m{marker} {title}\x1b[0m");
            }
            StreamEvent::Text(t) => {
                if self.thinking_open {
                    println!("\x1b[0m");
                    self.thinking_open = false;
                }
                if !self.text_started {
                    self.text_started = true;
                }
                print!("{t}");
                let _ = std::io::stdout().flush();
            }
            StreamEvent::ToolCallStart { name, .. } => {
                if self.thinking_open {
                    println!("\x1b[0m");
                    self.thinking_open = false;
                }
                println!("\n\x1b[36m→ {name}\x1b[0m");
            }
            StreamEvent::ToolCallUpdate { .. } => {}
            StreamEvent::ToolCallResult {
                name,
                summary,
                is_error,
                ..
            } => {
                if is_error {
                    println!("\x1b[31m✗ {name}\x1b[0m \x1b[90m{summary}\x1b[0m");
                } else {
                    println!("\x1b[32m✓ {name}\x1b[0m \x1b[90m{summary}\x1b[0m");
                }
            }
            StreamEvent::ApprovalNeeded {
                tool_name, summary, ..
            } => {
                println!("\n\x1b[33m? approve {tool_name}\x1b[0m \x1b[90m{summary}\x1b[0m");
            }
            StreamEvent::QuestionNeeded { prompt, .. } => {
                println!("\n\x1b[36m? {prompt}\x1b[0m");
            }
            StreamEvent::ModelSubstituted { served, .. } => {
                if self.thinking_open {
                    println!("\x1b[0m");
                    self.thinking_open = false;
                }
                println!("\n\x1b[33m! The selected model was unavailable; this response used {served} instead.\x1b[0m");
            }
            StreamEvent::ResumeHandle(_) => {}
        }
    }
}
