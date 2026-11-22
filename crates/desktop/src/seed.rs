//! Deterministic fake conversation content: varied Markdown with code, tables and lists.

fn rng(seed: usize) -> impl FnMut() -> usize {
    let mut s = (seed as u64).wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    move || {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (s >> 33) as usize
    }
}

const TOPICS: &[&str] = &[
    "the retry policy", "the session store", "the diff viewer", "the approval gate", "the provider registry",
    "worktree cleanup", "the streaming reducer", "quota accounting", "the command palette", "thread persistence",
];

const RUST: &str = "fn apply(events: &[Event], state: &mut State) -> Result<(), Error> {\n    for ev in events {\n        match ev {\n            Event::Text(t) => state.buf.push_str(t),\n            Event::Done => state.finish()?,\n            other => log::debug!(\"ignored {other:?}\"),\n        }\n    }\n    Ok(())\n}";
const TS: &str = "export function reduce(state: ChatState, ev: ChatEvent): ChatState {\n  switch (ev.type) {\n    case \"text_delta\":\n      return { ...state, tail: state.tail + ev.text };\n    case \"assistant_end\":\n      return { ...state, busy: false };\n    default:\n      return state;\n  }\n}";
const PY: &str = "def chunks(items, n):\n    \"\"\"Yield successive n-sized chunks.\"\"\"\n    for i in range(0, len(items), n):\n        yield items[i:i + n]\n\nprint(sum(1 for _ in chunks(list(range(100)), 7)))";
const JSON: &str = "{\n  \"provider\": \"mock\",\n  \"model\": \"zest-1\",\n  \"effort\": \"medium\",\n  \"tools\": [\"read_file\", \"bash\", \"apply_patch\"]\n}";
const SH: &str = "cargo test -p zest-core --lib -- agent::tests\nnpm run ui:e2e -- conversation-scroll\ngit status --short | head -20";

fn code(r: &mut impl FnMut() -> usize) -> String {
    let (lang, body) = match r() % 5 {
        0 => ("rust", RUST),
        1 => ("ts", TS),
        2 => ("python", PY),
        3 => ("json", JSON),
        _ => ("bash", SH),
    };
    format!("```{lang}\n{body}\n```")
}

fn table(r: &mut impl FnMut() -> usize) -> String {
    let rows = 3 + r() % 3;
    let mut s = String::from("| Step | Owner | Status | Notes |\n| --- | --- | --- | --- |\n");
    for i in 0..rows {
        s.push_str(&format!(
            "| {} | {} | {} | `{}` takes {} ms |\n",
            i + 1,
            ["core", "desktop", "ui", "cli"][r() % 4],
            ["done", "in review", "blocked"][r() % 3],
            TOPICS[r() % TOPICS.len()].replace(' ', "_"),
            20 + r() % 900
        ));
    }
    s
}

pub fn assistant_text(i: usize) -> String {
    let mut r = rng(i + 1);
    let topic = TOPICS[r() % TOPICS.len()];
    let mut s = format!("## Notes on {topic}\n\n");
    s.push_str(&format!(
        "I looked at {topic} and the short version is that it **mostly works**, but there are *two* edge cases worth fixing. \
         See [the design doc](https://example.com/docs/{i}) for context; the relevant helper is `reconcile_{}()`. \
         It is easy to miss because the failure only shows up after a long session, which is why the first report \
         described it as \"flaky\" rather than \"wrong\".\n\n",
        i % 17
    ));
    s.push_str("What I would do next:\n\n");
    for k in 0..(2 + r() % 4) {
        s.push_str(&format!("- Handle the case where {} is empty (see item {k})\n", TOPICS[r() % TOPICS.len()]));
    }
    if r() % 3 != 0 {
        s.push_str("\nSteps in order:\n\n1. Reproduce with the fixture.\n2. Add a regression test.\n3. Fix and re-run `npm run verify`.\n");
    }
    if r() % 2 == 0 {
        s.push('\n');
        s.push_str(&code(&mut r));
        s.push('\n');
    }
    if r() % 4 == 0 {
        s.push('\n');
        s.push_str(&table(&mut r));
    }
    if r() % 5 == 0 {
        s.push_str("\n> Heads up: this changes the persisted format, so older threads need a migration.\n");
    }
    s.push_str(&format!("\nLet me know if you want me to take {topic} first or start with the migration.\n"));
    s
}

pub fn user_text(i: usize) -> String {
    let mut r = rng(i + 90_001);
    let topic = TOPICS[r() % TOPICS.len()];
    match r() % 3 {
        0 => format!("Can you look into {topic}? It feels slow after a long session."),
        1 => format!("Why does {topic} behave differently in a split pane?\nPlease keep the answer short."),
        _ => format!("Add a test for {topic} and explain what it covers."),
    }
}

/// A long, steady Markdown stream: mixed blocks, repeated until `chars` are produced.
pub fn stream_script(chars: usize) -> String {
    let mut s = String::new();
    let mut i = 7000;
    while s.len() < chars {
        s.push_str(&assistant_text(i));
        s.push('\n');
        i += 1;
    }
    s
}

/// ~1 token per word plus its trailing whitespace.
pub fn tokenize(s: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut seen_space = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            seen_space = true;
            cur.push(ch);
        } else {
            if seen_space {
                out.push(std::mem::take(&mut cur));
                seen_space = false;
            }
            cur.push(ch);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

// ---- agent-shaped turns: reasoning and tool rows ----

pub struct ToolSeed {
    pub name: &'static str,
    pub summary: String,
    pub diff: Option<String>,
    pub is_error: bool,
    pub delay_ms: u64,
}

const FILES: &[&str] = &[
    "crates/core/src/agent.rs", "crates/core/src/provider/mod.rs", "crates/coordinator/src/queue.rs",
    "crates/desktop/src/workspace.rs", "crates/cli/src/serve.rs", "docs/features/delegation.md",
];

pub fn tool_seed(i: usize) -> ToolSeed {
    let mut r = rng(i + 500_003);
    let file = FILES[r() % FILES.len()];
    match r() % 4 {
        0 => ToolSeed {
            name: "read_file",
            summary: format!("{file}\n{}", (1..=14).map(|n| format!("{n:>4}  {}", RUST.lines().nth(n % 9).unwrap_or(""))).collect::<Vec<_>>().join("\n")),
            diff: None,
            is_error: false,
            delay_ms: 60,
        },
        1 => {
            let failed = r() % 5 == 0;
            ToolSeed {
                name: "bash",
                summary: if failed {
                    "cargo test -p zest-core\ntest agent::tests::retries_once ... FAILED\n\nfailures:\n    agent::tests::retries_once\n\ntest result: FAILED. 127 passed; 1 failed".to_string()
                } else {
                    format!("cargo test -p zest-core\nrunning {} tests\ntest result: ok. {} passed; 0 failed; 0 ignored", 100 + r() % 40, 100 + r() % 40)
                },
                diff: None,
                is_error: failed,
                delay_ms: 120,
            }
        }
        2 => {
            let diff = format!(
                "--- a/{file}\n+++ b/{file}\n@@ -{a},7 +{a},9 @@\n     let mut state = State::default();\n-    for ev in events {{\n-        state.apply(ev);\n-    }}\n+    for ev in events {{\n+        if ev.is_noise() {{\n+            continue;\n+        }}\n+        state.apply(ev);\n+    }}\n     state.finish()",
                a = 10 + r() % 200
            );
            ToolSeed { name: "apply_patch", summary: format!("patched {file}"), diff: Some(diff), is_error: false, delay_ms: 80 }
        }
        _ => ToolSeed {
            name: "grep",
            summary: format!(
                "reconcile\n{}",
                (0..(3 + r() % 6)).map(|_| format!("{}:{}: fn reconcile_{}(", FILES[r() % FILES.len()], 10 + r() % 400, r() % 9)).collect::<Vec<_>>().join("\n")
            ),
            diff: None,
            is_error: false,
            delay_ms: 40,
        },
    }
}

pub fn think_text(i: usize) -> String {
    let mut r = rng(i + 700_007);
    let topic = TOPICS[r() % TOPICS.len()];
    let mut s = format!("The user is asking about {topic}. ");
    for _ in 0..(4 + r() % 6) {
        s.push_str(&format!(
            "I should check how {} interacts with {} before changing anything, because the last time this broke it was a stale cache rather than the logic itself. ",
            TOPICS[r() % TOPICS.len()],
            TOPICS[r() % TOPICS.len()]
        ));
    }
    s
}
