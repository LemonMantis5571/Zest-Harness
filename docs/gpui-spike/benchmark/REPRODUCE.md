# Reproduce the Windows desktop comparison

Use the repo's pinned Rust and Node toolchains, npm dependencies, Windows SDK with the shader compiler, WebView2, Python and `psutil`. The commands below use `uv` to provide the Python dependency. Run from the repository root in PowerShell on an interactive Windows desktop.

```powershell
npm ci
Push-Location crates/desktop
& ../../node_modules/.bin/tauri.cmd build --no-bundle
Pop-Location
cargo build --locked --release --manifest-path crates/desktop/Cargo.toml

$tauriApp = (Resolve-Path target/release/zest-desktop.exe).Path
$gpuiApp = (Resolve-Path crates/desktop/target/release/zest-desktop.exe).Path
$results = Join-Path $env:TEMP ("zest-desktop-" + [guid]::NewGuid())
$bench = 'docs/gpui-spike/benchmark/compare.py'
uv run --with psutil $bench --tauri $tauriApp --gpui $gpuiApp --out "$results/default" --seconds 12
uv run --with psutil $bench --tauri $tauriApp --gpui $gpuiApp --out "$results/shell" --seconds 6 --shell
uv run --with psutil $bench --tauri $tauriApp --gpui $gpuiApp --out "$results/stress" --seconds 12 --stress
uv run --with psutil --with pywin32 python -B $bench --tauri $tauriApp --gpui $gpuiApp --out "$results/graphics" --seconds 12 --gpu --repetitions 2
cargo run --locked --release --manifest-path crates/desktop/Cargo.toml --example workbench_io
```

The additional frontend-specific checks exercise long code blocks, the GPUI reasoning/tool transcript and the native workspace.

```powershell
Push-Location $results
& $gpuiApp --bench --agent --history=5000 --rate=75 --secs=12
& $gpuiApp --selftest-workspace
Pop-Location
Push-Location crates/desktop/ui
$env:ZEST_PERF = '1'
& ../../../node_modules/.bin/playwright.cmd test streaming-performance --workers=1
Remove-Item Env:ZEST_PERF
Pop-Location
```

If Cargo uses a shared target directory, pass the release executables from that directory. Each output path must be new. It contains the per-trial JSON lines, app logs, synthetic inputs and isolated WebView2 profiles. The stress case explicitly loads all 5,000 messages into each renderer and needs at least 8 GiB of available memory. Tauri normally opens only the latest 10 user turns.

## What the harness measures

- Each workload runs one app at a time. Starting order alternates within and across repetitions. Private bytes, working set and cumulative process CPU cover the app and its child processes sampled about every 100 ms. Per-trial memory is the median of the samples in the interior of the timed phase. CPU is CPU seconds per elapsed second over that interval. Summed working sets can double-count shared pages.
- The release GPUI executable runs its existing transcript and scroll fixtures. The Tauri host runs a production-mode build of the actual React UI in WebView2. A separate Vite build selects the offline backend for the measurement page. The driver uses a dedicated WebView2 profile and a loopback page. Production assets, provider settings and release entry points do not need benchmark hooks.
- Both renderers use the same Rust text generator, Markdown and token boundaries. Each web trial checks its initial row count. A stream must render an answer and a scroll must move the viewport. Web events enter the actual chat reducer. GPUI receives events through the mock provider bridge. GPUI's last timer tick may deliver a few extra tokens.
- The transcript fixture uses a 1,000 × 900 logical GPUI window. The WebView2 comparison requests a 1,000 × 900 CSS viewport. Window chrome, sidebar and composer leave different amounts of space for messages. The empty-workspace comparison uses the full native GPUI workspace and the Tauri shell with an empty offline project.
- Web frame rate and frame gaps come from animation-frame callbacks. GPUI reports its own draws, deadlines and CPU draw time. Calling those counters the same metric would hide differences in what each renderer measures. The first-window timestamp is the first sampled visible titled app window. It can precede an interactive workspace and has roughly 100 ms sampling resolution.
- The accessibility probe attaches Windows UI Automation after the idle interval, then counts controls and names and checks the message editor's edit patterns. It runs in disposable offline windows. A screen reader and actual IME are separate end-user checks.
- The separate GPU pass reads Windows Performance Data Helper counters once per second. Within the measured process tree it sums utilization by physical GPU engine, takes the busiest engine at each sample and reports the median. Dedicated and shared video memory are summed for the app and its child processes across adapters. The measurements exclude GPU work attributed to Desktop Window Manager and provide no power reading.
- The shared Rust example creates 100 chats including a transcript over 8 MiB, then checks saves, reopening, sidebar summaries, windowing, cold and warm text search, and diff summaries. Its use of a new project per repetition guarantees an empty search index. Filesystem caches stay warm across operations.

The [workflow inventory](COVERAGE.md) describes which results transfer to the full product. The [native window smoke](../src/workspace_test.rs) and React browser tests cover tasks that the matched renderer workload does not time.
