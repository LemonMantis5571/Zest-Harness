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
