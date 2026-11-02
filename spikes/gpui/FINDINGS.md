# GPUI spike: could it host Zest's chat UI on Windows 11?

Throwaway code in this folder (about 3,400 lines of Rust). Run on 2026-11-02 in two rounds (the spike, then a follow-up on the parts where GPUI looked like a win; see "Round 2"), Windows 11 Pro 26200, Ryzen 7 8700G
(16 logical CPUs), RTX 3060 Ti + Radeon 780M, one 60 Hz display, MSVC (VS 2022 17.14), Windows SDK 10.0.26100, rustc 1.97.1.
**Caveat for every number below: a game (Fortnite) and a Discord clip-recording process were running on the machine the whole time.**
Words like "ran", "measured", "passed" mean I executed it. "Docs/code say" means I read it and did not run it.

## Verdict: go with caveats

GPUI can host the chat transcript and composer on Windows, and the riskiest rendering questions came out well: a virtualized 500-message transcript with a live Markdown stream drew in about 1.6 ms of CPU
per frame at 75 tokens/s (60 fps, no late frames), selection across blocks, copy, stick-to-bottom, a wrapping composer with image paste, a Tokio bridge, `zest-core`'s streaming types and an embedded webview all worked.
It is not a drop-in swap. Everything the webview gives for free had to be hand-built (Markdown, text editing, selection, syntax highlighting, accessibility), Zed's own Markdown and editor code is GPL and unusable,
the browser and HTML artifacts still need a webview, and two things are **unverified and carry the real risk**: a real CJK IME and a screen reader (see "Not verified").
The webview baseline says the existing UI already holds 60 fps at its real default window, so performance alone does not justify this; the measurable difference is footprint (about 138 MB private in one process against about 725 MB across about 12, not like-for-like) and headroom for long histories.
My recommendation: technically unblocked, not yet justified. Do the IME and screen-reader checks and decide what problem a native UI must solve before spending the porting effort below.

Round 2 asked whether the wins survive a realistic agent transcript (reasoning and tool rows between the messages, expandable). They did: draw time, CPU and memory were unchanged within run-to-run noise at 500 and 5,000 rows, and expanding rows behaves correctly at the tail and when scrolled up (see "Round 2").
It does not change the verdict: the wins are footprint and headroom for long histories, not frame rate. A piecemeal port is not what I tested: the working hybrid is a GPUI window that hosts child webviews, and those paint above GPUI content. I did not try the reverse, GPUI inside Tauri's window (GPUI creates its own windows).

## Setup (setup friction is a result)

| Item | What happened |
| --- | --- |
| crates.io | `gpui` is **0.2.2, published 2025-10-22**. `gpui_platform`, `gpui_windows`, `gpui_wgpu` and `gpui_tokio` are **not published** (crates.io says "does not exist"). `gpui_macos 0.1.1` is (2026-10-11). So the docs' `gpui_platform::application()` cannot be used from crates.io. |
| What I used | Git dependency on `zed-industries/zed` at `7a349ce42df27b03c3ac16d478c88b8e1411607e` (main, 2026-11-02): `gpui` + `gpui_platform` (`features = ["profiler"]`). Landing-page `Application::new()` is the 0.2.2 API; crate README `gpui_platform::application()` is main. They describe different code. |
| crates.io 0.2.2 | **Builds and runs on Windows** (scratch hello-world with `Application::new()`, 5m02 release build, window rendered). So crates.io is stale, not broken. But the published source has none of: `FollowMode`/`is_following_tail` (stick-to-bottom), `accesskit`/`role`/`aria_*` (any accessibility API), or the frame profiler (grep of the 0.2.2 source). The spike's approach would not work on it. |
| Build | Cold dev build 3m21s, release 1m47s on top, +2m04 with `zest-core`, +1m21 with `wry`. First fetch clones the whole Zed repo (~410 MB in `~/.cargo/git/db`). `target/` reached 4.7 GB. |
| Toolchain snags | None. MSVC + Windows SDK were already installed. **Release builds need `fxc.exe`** (`gpui_windows/build.rs` looks at `GPUI_FXC_PATH`, then `PATH`, then the Windows SDK, then panics with "Failed to find fxc.exe"); debug builds instead compile `shaders.hlsl` at runtime from the cargo checkout. No DX12/Vulkan/wgpu: the Windows renderer is DirectX 11 + DirectComposition, text is DirectWrite (code says). |
| Rust version | The repo pins 1.97.1 in `rust-toolchain.toml`; stable was also 1.97.1, so no conflict. |

The spike is a standalone crate: its own `[workspace]`, `Cargo.lock`, and `.gitignore` for `target/`. Nothing outside `spikes/gpui/` changed (`git status` shows only `spikes/`).

## Reuse from Zed (asked: is any of it usable outside Zed?)

- **Zed's `markdown`, `markdown_preview`, `editor`, `ui`, `ui_input`, `language`, `theme` crates are GPL-3.0-or-later** (their `Cargo.toml`). Zest is MIT. They also depend on each other and on Zed's `settings`/`theme`. Not reusable, even if they could be built standalone.
- `gpui` itself and its `examples/` are Apache-2.0 and are fine to copy from. I used `input.rs` (IME hooks), and read `list_example.rs`, `uniform_list.rs`, `text_wrapper.rs`. `uniform_list` is fixed-height only, so chat needs `list` + `ListState` (variable heights, `ListAlignment::Bottom`, `FollowMode::Tail`). `text_wrapper.rs` is an ellipsis/wrapping demo, not a text engine. `input.rs` is single-line (`ShapedLine`); I rebuilt it multi-line on `StyledText`.
- GPUI has **no Markdown, no syntax highlighting, no text field, no webview**. I used `pulldown-cmark` (MIT) and `syntect` (MIT, pure-Rust regex backend).

## Experiment 1: streaming transcript

**What worked (ran):** 500 seeded messages in a virtualized `list`, one live assistant message streaming at a target of 75 tok/s,
rendered as Markdown: headings, bold/italic/strike, inline code, links (click opens URL), bullet/numbered lists, blockquote,
fenced code with syntax colours and a Copy code button, tables, rules. A screenshot of the real window confirmed it looks right.
In-process end-to-end checks (`--selftest`, events injected through `Window::dispatch_event`, real Windows clipboard) **all passed**:

| Check | Result |
| --- | --- |
| Stick to bottom while streaming | passes: `FollowMode::Tail` is built in, no code of mine |
| Scroll up releases the tail; "Jump to latest" re-engages | passes (wheel event released it; button re-followed) |
| Drag-select across blocks, Ctrl+C | passes: selection spanned unit 1 to unit 28 of one message, 930 chars copied |
| Copy of a selection reaching off-screen messages | passes: 20,383 bytes from 41 messages, copied from the document model |
| Link click | passes |

**Hand-built (lines of Rust):** Markdown to document model plus syntect (`md.rs`, 327); element renderer, selectable text and the
cross-block selection state (`view.rs`, 380); transcript, mouse selection layer, stream reducer (`main.rs`, about 300 of 615).
Selection works like this: each text run is a `SelText` element that wraps `StyledText` and records its `TextLayout` when painted.
Mouse handlers hit-test those layouts and store `(message, unit, byte offset)` anchors. Copy reads the model, so off-screen text still copies.
Zed has no equivalent you can borrow.

**Measured (release build, 60 Hz display, 500 messages, live message streaming at 75 tok/s for 30 s and ending at 12.7 KB; two runs):**

| Metric | Run 1 | Run 2 |
| --- | --- | --- |
| Frames drawn (60 Hz, GPUI's on-time check) | 1,800, 0 late | 1,799, 0 late |
| `Window::draw` CPU time mean / p95 / p99 / max | 1.64 / 2.58 / 3.09 / 4.15 ms | 1.63 / 2.65 / 3.19 / 4.42 ms |
| First invalidation to present, mean / p95 / max | 10.2 / 16.6 / 18.3 ms | 10.2 / 16.6 / 18.2 ms |
| Markdown reparse of the live message p50 / p95 / max | 0.12 / 0.24 / 1.3 ms | 0.12 / 0.26 / 1.5 ms |
| Tokio thread to UI wake-up latency p50 / p95 / max | 0.20 / 1.2 / 3.4 ms | 0.19 / 1.2 / 3.8 ms |
| Process CPU, average (peak) in cores | 0.13 (0.30) | 0.14 (0.34) |
| Working set / private bytes (peak) | 129 / 138 MB | 129 / 138 MB |
| GPU engine utilisation (Windows per-process counters, 1 Hz) | 0% | 0% |

Other loads (same machine, same build):
- 200 tok/s for 20 s, live message ends at 22.5 KB: draw mean 2.45 / p99 5.1 / max 6.1 ms, 1,200 frames, 0 late, 0.20 cores, 134 MB.
- 5,000-message history, 75 tok/s for 30 s: draw mean 1.58 / p99 3.9 / max 5.2 ms, 0 late, 0.13 cores, 143 MB working set (+14 MB over 500 messages). The virtualized list scales with the viewport, not the history.
- Scrolling programmatically through the 500-message history for 20 s (no streaming): draw mean 0.78 / p99 1.15 / max 7.8 ms, 0.05 cores, 122 MB.
- Draw time grows with the live message length (1.1 ms mean at 6 KB, 1.6 ms at 12.7 KB, 2.45 ms at 22 KB) because the live message is re-laid-out and re-shaped each frame.
- Seeding 500 messages (parse + highlight all) took 105-118 ms; syntect init 1.3 ms. The seeded history repeats code blocks and a content-keyed highlight cache dedups them, so 5,000 messages took only 140 ms: do not extrapolate seed time.

**Measurement notes.** All numbers come from `--bench`, which uses GPUI's own `profiler` feature (`Window::frame_duration_snapshot`) for frame times and a PowerShell sampler for CPU/memory/GPU.
GPUI throttles *inactive* windows to about 30 fps by default (`WindowOptions::inactive_frame_interval`, 33.3 ms, confirmed in the source). My automated runs do not take focus, so my first runs drew 30 frames/s;
the numbers above set that interval to `None` and run at the full 60 Hz. A game (Fortnite) and a Discord clip recorder were running throughout, so treat every figure as measured under ambient load.
The GPU counters read 0% on every sample: GPU time is below what 1 Hz per-process counters resolve. The final runs stream through zest-core's `Provider::stream_turn` (see Experiment 3); the earlier no-core runs measured the same.

**Webview baseline (ran, by a delegated agent; I spot-checked the raw JSON for four of its figures).** The real Zest React app (production build, headed Chromium 153, 1280x812, 60 Hz) driven through its own
chat-event handler against the offline fixture backend, with a JS port of `seed.rs` whose output I compared byte-for-byte with the Rust (500 messages, the same 12.7 KB stream at 75 tok/s for 30 s, one `text_delta` per token).
No tracked file was edited; scripts and per-run JSON are in the session scratchpad, not in this repo. Edge 156 (the WebView2 engine) gave the same picture on spot runs.

**This is not a like-for-like comparison, and it is not a speed claim.** The webview runs the whole app (sidebar, trace rows, checkpoint rail, composer) with no Tauri IPC; the spike draws only a transcript and a composer.
The real app does **not** virtualise: `MessageScroller` renders every loaded message and deliberately has no `content-visibility`; the backend instead windows the thread to the last 10 user turns (`THREAD_WINDOW_USER_TURNS`, 20 more per "Earlier turns" click).
So the realistic webview case is 20 messages, and 500 in the DOM is a stress case that needs about 12 clicks.

| Whole process tree during the 30 s stream | Webview, 20 messages (3 runs) | Webview, 500 in the DOM (2 runs) | GPUI spike, 500 virtualized (2 runs) |
| --- | --- | --- | --- |
| Frame rate | 59.9-60 fps, p99 frame gap 16.8 ms, 0-2 frames over 25 ms | 33 fps, p99 gap 50 ms, 656-664 frames over 25 ms | 60 fps, 0 late |
| Main-thread / draw cost per frame | 4.0-4.5 ms (React, JS, style, layout, paint, commit) | about 30 ms, main thread 99% busy | 1.6 ms mean, 3.1-3.2 ms p99 (`Window::draw` only) |
| CPU, average cores | 0.49-0.54 | 1.54-1.58 | 0.13-0.14 |
| Working set / private bytes (peak) | 984-1,013 / 705-725 MB, about 12 processes | 1,375-1,439 / 1,088-1,148 MB | 129 / 138 MB, one process |
| GPU engine % average | 0.5-0.7 | 0.5-1.0 | 0 (below resolution) |

The two draw/main-thread figures measure different things and must not be subtracted. What the numbers do support: at the real default window the webview holds 60 fps under this stream, so
**there is no frame-rate problem for a native UI to fix in the default case**; footprint is where they differ (private bytes and CPU in the table, again not like-for-like); and the DOM approach degrades with loaded history where the virtualized `list` did not (GPUI at 5,000 messages: mean draw 1.58 ms).

**Side finding in the existing UI (not part of the spike question):** at 500 messages the agent counted 250 running `animate-spin` animations, one per settled assistant message, from `MarkdownActions`' `IconSwap`, which keeps a `Loader2Icon animate-spin` mounted while hidden (confirmed in `MarkdownActions.tsx`). With 500 messages loaded the idle app used about 1.1 cores and about 95% main-thread busy;
with the animation paused by injected CSS, idle dropped to 0-0.07 cores and the streaming frame rate rose from 33 to about 48 fps. At 20 messages idle is about 0.1 cores against 0.01 for the empty app; I did not isolate how much of that is the same cause. This is worth a separate fix whatever happens with GPUI.


**Gaps / not built:** word and line selection (double/triple click), auto-scroll while dragging is only a nudge on mouse-move, images and the lightbox,
Mermaid (I did not look for a Rust renderer; Zed's `mermaid_render` is GPL-3.0), link-preview cards, "Copy Markdown"/"Save as", block-streaming reveal, resumable highlighting (I re-highlight the whole fence;
a cache hides it for settled blocks), approval and question cards, subagent and shimmer rows. (Round 2 added reasoning and tool rows and fixed code and tables under list items.)
**Grammar coverage:** see "Grammar packs" under Round 2. (Round 1 said syntect covers 17 of Zest's 30 shiki grammars; that count was wrong, see the correction there.)

**Stability finding:** a panic inside a GPUI window event handler **aborts the process** (non-unwinding callback). My first drag-select
panicked on a `RefCell` double borrow and killed the app. Rust UI code on this path cannot rely on unwinding to survive a bug.

## Round 2: do the wins survive a realistic agent transcript? (ran)

Round 1's transcript was plain Markdown messages. A coding agent's transcript is mostly small rows between the messages, so round 2 added them and re-measured. I did not check what else was running on the machine this time.
**Built (about 750 lines of Rust, 220 of them the self-test):** collapsible reasoning and tool rows in the same virtualized list, fed by `StreamEvent::Thinking`, `ToolCallStart` and `ToolCallResult` through the same `Provider::stream_turn` mock (`bridge.rs`, `main.rs`, `view.rs::render_trace`);
a tool row's body is a diff or text block through the existing renderer, so selection, highlighting and "Copy code" work in it with no extra code; code and tables under list items now keep their indent (`md.rs`); `--agent` seeds history as user, reasoning, three tool rows, answer; `--open` starts with every row expanded.
Status is static text (running, tick, cross), not a spinner: a native spinner needs a redraw every frame while it is on screen, which is the webview's hidden-spinner mistake again if it stays on a settled row.

**Checks (`--selftest --agent`, all PASS; the 15 round-1 checks still PASS):**

| Check | Result |
| --- | --- |
| Nested blocks under list items | code and table carry indent 1, two items |
| History shape | 500 rows: 84 reasoning, 249 tool |
| Live turn through the bridge | rows arrive as reasoning, three tool rows, answer; each tool row gets its result title |
| Click a tool header while following the tail | opens (29 px to 197 px), the tail is still followed, the last item's bottom edge is at the viewport bottom (814 px) |
| Click a header while scrolled up | the row's top edge stays at 135 px, height 29 px to 217 px, the tail stays released |
| Copy across a collapsed reasoning row | 1,058 chars without its text; 1,827 chars once expanded |

While following the tail, expanding a row above the bottom keeps the bottom pinned, so the expanded row's top edge must move up by the growth (inferred from the pinned bottom; the test did not log the top). I did not compare that with the webview.
**API note:** `ListState::bounds_for_item` returns `None` for every item while the list follows the tail (there is no scroll anchor, so the logical scroll top is the end; read from `list.rs`). Anything that must locate a row at the tail (click targets, anchored popovers) has to record painted bounds itself; the spike does it with a `canvas` per row.

**Measured (release, 60 Hz, live message streaming at 75 tok/s for 30 s, all draw times in ms):**

| Transcript, build | Draw mean / p95 / p99 / max | Late frames | CPU cores avg | Private MB |
| --- | --- | --- | --- | --- |
| 500 plain, round-1 code (rebuilt from the commit), 2 runs | 2.71 / 4.87 / 6.27 / 8.83 and 2.90 / 5.64 / 7.14 / 10.29 | 0, 0 | 0.21, 0.24 | 139.4, 139.1 |
| 500 plain, round-2 code, 2 runs | 2.55 / 4.48 / 5.52 / 8.00 and 3.00 / 5.80 / 7.14 / 10.44 | 0, 0 | 0.24, 0.25 | 139.6, 140.2 |
| 500 agent-shaped rows, round-2 code, 4 runs | means 2.79 to 3.09, p99 6.23 to 7.49, max 8.4 to 23.6 | 0, 0, 1, 1 | 0.22 to 0.27 | 138.2 to 138.7 |
| 5,000 agent-shaped rows | 2.97 / 5.12 / 6.23 / 9.10 | 0 | 0.24 | 150.9 |
| 5,000 plain | 2.82 / 5.21 / 6.53 / 9.52 | 0 | 0.26 | 153.7 |

Programmatic scroll through 500 rows for 20 s, one run each, no streaming: plain mean 1.27 / p99 3.32 / max 11.35; agent rows 1.45 / 3.19 / 12.30; agent rows all expanded 1.30 / 3.12 / 17.47 (about 700 frames each).
Reading: the rows did not move any number beyond the spread between two runs of the same build. The late frames (1 each in two agent runs, 18 and 24 ms worst) are in the range the plain runs' max already reach; I did not isolate them.
**The machine drifted:** the round-1 code measured 1.6 ms mean draw and 0.13 cores in round 1 and 2.7 to 2.9 ms and 0.21 to 0.24 cores today, from the same source. Only compare numbers inside one table. The round-1 table above and this one are not comparable, and the webview baseline was measured in round 1.
Not measured: a long-lived session (growth over hours), many expanded diffs at once beyond the scroll case above, images. Seeding 500 rows took 115 to 220 ms (more rows add little because the highlight cache dedups the repeated content); parsing tool bodies lazily on first expand would avoid it and was not built.

### Grammar packs (ran)

**Correction to round 1:** it said syntect's default pack natively covers 17 of Zest's 30 grammars. That count looked Zest's names up as syntect tokens (`csharp`, `shell`), which missed C# and Shell. With the right tokens (`cs`, `bash`) the default pack covers 19 natively (`--langs`).

| Pack | Grammars | Zest's 30: native / approximate / missing | Init | Private MB (500 agent rows) | Seed 500 rows | Exe |
| --- | --- | --- | --- | --- | --- | --- |
| syntect default | 75 | 19 / 4 (jsx, powershell, tsx, typescript, shown as JavaScript or Bash) / 7 (docker, graphql, kotlin, scss, swift, toml, vue) | 2.5 ms | 138.6 | about 150 ms | 16.5 MB |
| plus `two-face` 0.5.2 (bat 0.26.1), `--features grammars` | 213 | 28 / 2 (jsx, powershell) / 0 | 4.1 ms | 181.4 to 182.8 | about 303 ms | 17.1 MB |

Draw time and CPU were unchanged with the larger pack (mean 2.88 and 2.85 against 2.79 and 3.09 for the default, alternating runs), so it closes the coverage gap at a cost of about 43 MB private memory and double the seed time, which eats a visible share of the footprint win.
A 256 ms frame appeared once in an earlier run with the larger pack and did not come back in the next two; I did not isolate it. Bundling means shipping bat's syntax licenses: `two-face` is MIT or Apache-2.0, its embedded syntaxes carry their own licenses and it exposes them through `acknowledgement::listing()`. I did not review them.

## Experiment 2: composer

**What worked (ran, in-process):** multi-line text that wraps and grows (4 lines/88 px to 6 lines/132 px after two Shift+Enter, capped at 8 lines then scrolls),
Enter sends and clears, Shift+Enter inserts a newline, mouse drag selection, Shift+Left, Ctrl+A/C/X, Up/Down by visual line, Home/End by logical line,
**caret movement by grapheme** (one Left key crossed a 5-codepoint family emoji), paste of text (CRLF normalised) and paste of an **image** through the
real Windows clipboard (PNG written, read back, attached as a thumbnail chip). Each of these is a PASS line in `--selftest`.
`EntityInputHandler` is implemented for composition, but only exercised with simulated input (see IME below).

**Hand-built:** `composer.rs`, 615 lines.

**IME, dead keys, emoji picker (partly verified, the real-IME part is not):**
- **Real IME: not tested.** No CJK IME is installed (only two Spanish keyboard layouts), and installing a language pack changes system settings.
- **Real keystrokes (dead keys, AltGr, Win+.): not run.** I wrote a guarded injector that only types when the spike window is the foreground window and the user has been idle 90 s, and waited 25 minutes; the machine's user was never idle that long and a game was running, so I did not steal focus or inject input.
- **What I ran instead:** `WM_CHAR` messages posted to the spike's own window handle, i.e. exactly what Windows delivers after the keyboard layout has resolved a dead key or AltGr. `U+00E1 á`, `U+00FC ü`, `@`, `€`, `あ` and a UTF-16 surrogate pair (`U+1F600`, what an emoji picker would send) all reached `replace_text_in_range` intact and rendered, with the colour emoji drawn by DirectWrite. That covers GPUI's side of dead keys and emoji. It does **not** show that the layout composes the dead key, or that the Windows emoji panel inserts through `WM_CHAR` (it may use TSF).
- **Code says:** the Windows backend is IMM32 only: `WM_IME_COMPOSITION` with `GCS_COMPSTR` calls `replace_and_mark_text_in_range`, `GCS_RESULTSTR` calls `replace_text_in_range`, the composition and candidate windows are placed from the caret via `ImmSetCompositionWindow`/`ImmSetCandidateWindow`, IME is turned on/off per focused element through `ImmAssociateContextEx`, and there is a special case for Japanese IME sending `lparam = 0`. `WM_DEADCHAR` is not handled by GPUI (the OS composes). The marked range draws as an underline in my composer. Modern TSF-only features (reconversion, handwriting, voice typing, some emoji-panel paths) go through the IMM compatibility layer or do not work; I could not check which.

**Is the `input.rs` / `EntityInputHandler` approach enough?** It is enough for the platform text-input protocol: GPUI routes WM_IME_COMPOSITION to
`replace_and_mark_text_in_range` / `replace_text_in_range`, and positions the candidate window from `bounds_for_range` (code says; IMM32, not TSF).
It is not an editor. Missing and hand-written if wanted: undo/redo, word navigation, double/triple click, caret blink, scroll-into-view verification,
autocomplete popups (slash commands, mentions), drag-and-drop of files, text-selection semantics for assistive technology. **Zed's `editor` is not an option**
(GPL-3.0, tied to Zed's buffer/display-map crates), so a real editor component would be your own or a third-party one; none was evaluated.
GPUI's `show_character_palette()` is a no-op on Windows (trait default; `gpui_windows` does not override it), so the emoji picker relies on the OS and IMM32 compatibility.

## Experiment 3: async backend bridge

**Pattern that works (ran):** a plain `std::thread` owns a multi-thread Tokio runtime. Commands go UI to Tokio over `tokio::sync::mpsc::unbounded_channel`
(its `send` is synchronous, callable from the UI thread). Events go Tokio to UI over `futures::channel::mpsc::unbounded`; the UI side is
`cx.spawn(async move |this, cx| while let Some(ev) = rx.next().await { drain with try_recv; this.update(cx, ...) })`.
The channel's waker is GPUI's foreground executor, so no manual "wake the UI" code is needed. Wake-up latency (timestamp on the Tokio thread, compared on the UI thread) was p50 0.19 ms, p95 1.2 ms, max 3.8 ms over 2,260 events.
Send Stop (Cancel) back works. Batching with `try_recv` before one `cx.notify()` keeps frames from multiplying.

**Pitfalls:** Tokio futures must be `Send + 'static`, GPUI entities and `cx` are neither, so only plain data crosses the channel. Do not create the runtime inside
the GPUI app thread. A paced stream reached the UI in about 64 batches/s (about 15.6 ms apart), which matches the default Windows timer granularity for `sleep(4ms)`, so the pacer catches up from a clock instead of counting sleeps.
Dispatching events from inside `WindowHandle::update` panics (`cannot update Root while it is already being updated`) because listeners update the same entity.
Zed also ships `gpui_tokio` (a `Tokio::spawn` wrapper, about 100 lines, **unpublished**); I did not need it.

**`zest-core` path dependency (ran, behind `--features core`):** it compiles on Windows with no credentials, adds about 118 crates (360 to 478 unique crate versions) and 2m04 to a release
build, and its own `Cargo.lock` stays inside the spike. A `MockProvider` implementing `zest_core::provider::Provider` drove the UI through the real
`Provider::stream_turn(&TurnRequest, &mut dyn FnMut(StreamEvent<'_>))` contract: 1,125 `Text` events over 15 s, mean draw 1.12 ms. So the provider-neutral
streaming types are usable from a native UI. What is **not** in core: Zest's `ChatEvent` mapping and the transcript reducer. The mapping lives in `crates/desktop` (Tauri) and the
reducer in `chatReducer.ts`, so a native UI must re-create both in Rust (`Thread::apply_*` in core is a start).

## Quick investigation: webview embedding

**Works (ran).** `impl HasWindowHandle for gpui::Window` exists, and wry 0.57's `WebViewBuilder::build_as_child(&window)` attached a WebView2 child
at a fixed rectangle inside the GPUI window (`--features webview --webview`). It rendered, laid over GPUI content, and showed up in the UI Automation tree as a `WRY_WEBVIEW` child
window. Cost: about 30 lines. **Limits (known from how child HWNDs work, not all run):** the webview is a native window that always paints above GPUI content,
so GPUI menus, tooltips, toasts and modals cannot overlap it, and it cannot be rounded or clipped; its bounds must be pushed from GPUI layout on every resize
(`set_bounds`); keyboard focus moves to the child HWND, so focus handoff with the composer is unverified. Today's agent browser is a separate Tauri `WebviewWindow` (`crates/desktop/src/browser.rs`),
which needs no embedding at all. HTML artifacts run arbitrary HTML/JS, so they need a webview in any design: child webview or separate window. The in-app browser panel was not in this
worktree (`BrowserPanel.tsx` is absent here), so I could not compare against it.

## Quick investigation: accessibility

Verified by dumping the Windows UI Automation tree (no screen reader involved): the window, a **Pane**, and anything I annotated by hand appear
(`Edit "Message"` with `ValuePattern` for the composer; `List "Conversation"` with `ListItem "Zest: Notes on..."`, only the rows painted this frame).
Unannotated text, headings, links, tables and code blocks are **invisible** to UIA. GPUI provides an ARIA-like API (`role`, `aria_label`, `aria_value`, `text!`)
and a `Text` element that is accessible, but my `StyledText`-based selectable text is not. Full content would need one accessible node per unit plus selection/caret
semantics. Narrator/NVDA behaviour, focus order and live-region announcements for streaming were **not tested**.

## Not verified

- **Real IME** (Japanese/Chinese/Korean): none installed. Only two Spanish keyboard layouts exist (`es-MX`/`es-419`). Installing a language pack changes system settings, so I did not. To test it: add Japanese (Microsoft IME) in Settings, run `zest-gpui-spike.exe --focus` from a terminal with `ZEST_IME_TRACE=1`, compose and commit; each `replace_and_mark_text_in_range`/`replace_text_in_range` call prints.
- Real keystrokes for dead keys, AltGr and the Win+. emoji panel (only the resulting `WM_CHAR` stream was tested), and the Shift+Enter / Enter behaviour while an IME is composing.
- Screen reader (Narrator, NVDA), focus order, high-DPI/mixed-DPI, multi-monitor, RTL/bidi, long-run memory growth, a multi-hour transcript, resize behaviour.
- Frame pacing without the game running; GPU load beyond "below counter resolution".
- Webview focus handoff, z-order against GPUI popups, and webview behaviour on resize.

## What would need porting

About 39,000 lines of TypeScript across 81 components, 28 Playwright specs, `zest-control` and the fixture backend do not carry over. Rough buckets:

1. **Rendering:** GFM with nested blocks, images (http, data, local assets), link-preview cards, lightbox, Mermaid, the 30-grammar highlighter, Trace rows (reasoning and tool rows are prototyped; chain-of-thought, subagents and shimmer are not), approval/question cards, working indicator, context meter, document-card framing, copy/save actions.
2. **Input:** attachments (chips, lightbox, limits), slash commands and palette popups, drafts, the queue panel, undo, word/line selection, drag-and-drop, a verified IME story.
3. **Shell:** sidebar with status groups, resizable nested split panes, workbench tabs, pickers (model/provider/effort), settings and themes, notifications, command palette, keyboard map.
4. **Webview-hosted:** agent browser, HTML artifacts (stay webview windows), anything that embeds third-party content.
5. **Backend glue:** replace the Tauri command surface and ts-rs bindings with in-process calls, port `chatReducer.ts` and the `ChatEvent` mapping to Rust, keep approval and credential boundaries on every path.
6. **Quality:** accessibility from scratch, a new test strategy (GPUI test contexts instead of DOM-driven Playwright), packaging, signing and updates without Tauri's bundler.
7. **Dependency risk:** pre-1.0, git-pinned to a 410 MB repo, docs that describe a different API than the published crate, panics abort.

## Reproduce

```bash
cd spikes/gpui
cargo build --release --features core,webview
target/release/zest-gpui-spike.exe                       # interactive
target/release/zest-gpui-spike.exe --selftest            # in-process checks, prints PASS/FAIL
target/release/zest-gpui-spike.exe --bench --rate=75 --secs=30   # prints BENCH {json}, writes bench-last.json
target/release/zest-gpui-spike.exe --scroll --secs=20    # scroll bench
target/release/zest-gpui-spike.exe --langs               # grammar coverage
target/release/zest-gpui-spike.exe --idle --webview --topmost    # child webview
target/release/zest-gpui-spike.exe --selftest --agent    # round 2: nested Markdown, reasoning/tool rows, expand/collapse, tail
target/release/zest-gpui-spike.exe --bench --agent       # agent-shaped history (add --history=5000)
target/release/zest-gpui-spike.exe --scroll --agent --open       # every row expanded
cargo build --release --features core,webview,grammars   # bat grammar pack; then --langs
```
Automated modes do not take focus. `--focus` forces activation. `ZEST_IME_TRACE=1` prints every text-input callback.
