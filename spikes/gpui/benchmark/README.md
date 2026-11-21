# Should Zest replace Tauri with GPUI?

**Keep Tauri as the default desktop app.** GPUI used roughly one third of Tauri's memory while streaming a normal chat and stayed smooth with 5,000 messages open. Replacing Tauri today would remove the integrated agent browser, interactive HTML artifacts, pull-request review controls, plugin management and the Windows and Linux installer pipeline. The native workspace also exposed 14 buttons without accessible names on Windows.

## Windows results, 21 November 2026

Each entry is the median of three independent process launches per frontend, alternating which ran first. Memory is private MiB summed across the app's process tree. CPU is average cores during the measured interval. Each transcript run lasted 12 seconds. Each empty workspace idled for six seconds before the separate accessibility probe.

| Workload | Tauri private MiB | GPUI private MiB | Tauri CPU cores | GPUI CPU cores |
| --- | ---: | ---: | ---: | ---: |
| Empty full workspace | 305 | 88 | 0.00 | 0.00 |
| Idle transcript, 20 messages | 375 | 129 | 0.00 | 0.01 |
| 20 messages, 75 tokens/s | 395 | 132 | 0.41 | 0.11 |
| 20 messages, 200 tokens/s | 403 | 133 | 0.43 | 0.13 |
| 500 messages, 75 tokens/s | 755 | 133 | 1.32 | 0.11 |
| Scroll through 500 messages | 788 | 130 | 0.70 | 0.04 |
| 5,000 messages, 75 tokens/s | 2,459 | 147 | 2.18 | 0.11 |

At 20 messages and either streaming rate, the WebView2 UI delivered 60 animation callbacks/s. GPUI drew about 59–60 frames/s on the 60 Hz display. At 500 messages, the WebView2 UI delivered 58.3 callbacks/s with a 33.4 ms p99 gap. At 5,000 it fell to 4.8 callbacks/s with a 667 ms p99 gap. GPUI still drew 59.4 frames/s. Tauri normally opens the latest **10 user turns** and pages older turns on demand. The 500- and 5,000-message measurements deliberately put every message in the rendered chat at once.

The release executable alone was 25.3 MiB for Tauri and 38.1 MiB for GPUI. Tauri used the machine's installed WebView2 runtime. The first visible titled window appeared in 103 ms for Tauri and 310 ms for GPUI in the empty workspace runs. This samples a window handle every ~100 ms. Time to first completed paint and time to an interactive composer require separate instrumentation.

In a separate GPU counter pass at 75 tokens/s with 20 messages, Tauri used 85 MiB of dedicated video memory and GPUI used 25 MiB. Median utilization of the busiest app-attributed GPU engine was 3.7% and 2.6%, respectively. With 5,000 messages it was 0.7% for Tauri and 2.5% for GPUI while the two delivered very different frame rates. Each GPU figure uses two launches per frontend.

## What a switch would cost

Windows UI Automation found **37 of 37 buttons named in Tauri and 0 of 14 in GPUI** in the empty workspaces. Both exposed the message editor. Tauri exposed Text and Value patterns and accepted a write/read round trip. GPUI exposed Value, and the write/read probe did not succeed. The probe used one UI Automation client. Narrator, NVDA and real CJK input still need end-user tests.

The native window's offline scenario covers sending, queue recovery, approvals, a typed question, attachments, a side chat, split, search, fork, committed diffs after reopening, file preview and the delegation worker/reviewer/apply path. The React fixture and CLI checks cover their respective frontends. Live provider/network speed, GPU energy, multi-hour memory growth, macOS and Linux graphics were outside this Windows run. The [feature-by-feature comparison](COVERAGE.md) accounts for all 38 feature-map entries.

## Operations both frontends share

Across three fresh projects, saving the 15.7 MiB, 5,000-message transcript took 57 ms and reopening it took 87 ms. Listing 100 chats took 13 ms. Building the index and locating the final answer took 1.20 s on first search. Repeating the search took 6.2 ms. Projecting the latest 20 messages took 0.09 ms. Summarizing an in-memory diff of 100 changed files took 0.11 ms. Both frontends use these Rust paths. These times exclude painting and Git subprocess startup.

The GPUI agent-shaped fixture also streamed through 5,000 rows with reasoning and tool activity. Its CPU draw time was 1.6 ms at p99. The React code-block fixture completed all ten growing fences with exact final text, taking 1.44 s for 20 KiB and 2.15 s for 100 KiB at the prescribed 40-chunk cadence. These are separate workloads.

## Measurement scope

The matched transcript workloads use the actual React UI in the Tauri WebView2 host and GPUI's transcript fixture. The full-workspace row uses GPUI's native offline workspace and the React shell with an empty offline project. The desktop windows include different controls, and the offline renderer measurements exclude Tauri command latency and real provider traffic. GPU draw time cannot be inferred from a browser animation callback. Each frontend's own counters are retained in the raw results.

The machine ran Windows 11 Pro build 26200, Ryzen 7 8700G, 27.6 GiB usable RAM, RTX 3060 Ti and Radeon 780M, a 60 Hz display and WebView2 158.0.4370.33. All timings are local and filesystem caches were warm. The workload uses no provider credentials. Source revision for the desktop apps was `85a876c`.

[Reproduce the trials](REPRODUCE.md) · [Per-run data](windows-2026-11-21.json)
