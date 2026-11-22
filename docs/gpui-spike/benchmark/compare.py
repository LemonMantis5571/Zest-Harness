"""Run from Windows with uv run --with psutil compare.py --help."""
import argparse
import ctypes
import json
import os
from pathlib import Path
import socket
import statistics
import struct
import subprocess
import time
import urllib.request

import psutil

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
CASES = [
    ("idle", 20, 0),
    ("stream", 20, 75),
    ("stream", 500, 75),
    ("stream", 20, 200),
    ("scroll", 500, 0),
]
USER32 = ctypes.windll.user32
ENUM = ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.c_void_p, ctypes.c_void_p)


def visible_window(pid):
    found = []
    def match(hwnd, unused):
        owner = ctypes.c_ulong()
        USER32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if owner.value == pid and USER32.IsWindowVisible(hwnd) and USER32.GetWindowTextLengthW(hwnd) > 0:
            found.append(hwnd)
            return False
        return True
    USER32.EnumWindows(ENUM(match), 0)
    return bool(found)


def sample(root, cpu):
    private = ws = count = 0
    for proc in [root, *root.children(recursive=True)]:
        try:
            mem = proc.memory_info()
            times = proc.cpu_times()
        except (psutil.NoSuchProcess, psutil.AccessDenied):
            continue
        cpu[proc.pid] = times.user + times.system
        private += mem.private
        ws += mem.rss
        count += 1
    return (time.perf_counter(), sum(cpu.values()), private / 2**20, ws / 2**20, count)


def port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def seed(exe, n, needed):
    data = subprocess.check_output([exe, str(n)])
    strings = []
    index = 0
    while len(strings) < n + needed:
        if index >= len(data):
            raise ValueError("The GPUI fixture exhausted its 80 KB stream. Reduce --seconds.")
        size, = struct.unpack_from("<I", data, index)
        index += 4
        strings.append(data[index:index+size].decode())
        index += size
    return {"history": strings[:n], "tokens": strings[n:]}


def stop(tree):
    try:
        family = tree.children(recursive=True)
    except psutil.NoSuchProcess:
        family = []
    for proc in [tree, *family]:
        try:
            proc.kill()
        except psutil.NoSuchProcess:
            pass
    psutil.wait_procs([tree, *family], timeout=3)


def trial(args, run_dir, engine, case, number):
    mode, n, rate = case
    seconds = args.seconds
    directory = run_dir / f"{number}-{engine}-{mode}-{n}-{rate}"
    directory.mkdir()
    env = dict(os.environ)
    scratch = directory / "tmp"
    scratch.mkdir()
    env.update(TEMP=str(scratch), TMP=str(scratch))
    args_app = [args.gpui] if engine == "gpui" else [args.tauri]
    ready = directory / "active"
    if engine == "tauri":
        endpoint = port()
        env.update(WEBVIEW2_USER_DATA_FOLDER=str(directory / "webview-profile"),
                   WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=f"--remote-debugging-port={endpoint}")
        case_path = directory / "workload.json"
        workload = dict(seed(run_dir / "seed.exe", n, rate * seconds), mode=mode, rate=rate, seconds=seconds)
        case_path.write_text(json.dumps(workload), encoding="utf-8")
    elif mode == "workspace":
        args_app.append("--demo")
    else:
        flag = {"idle": "--idle", "stream": "--bench", "scroll": "--scroll"}[mode]
        args_app.extend([flag, f"--history={n}", f"--secs={seconds}"])
        if rate:
            args_app.append(f"--rate={rate}")
    samples, cpu = [], {}
    gpu = None
    gpu_samples = []
    if args.gpu:
        from gpu import GpuSampler
        gpu = GpuSampler()
    driver = None
    accessibility = None
    start = time.perf_counter()
    first_window = active = None
    with (directory / "app.log").open("wb") as log, (directory / "webview.log").open("wb") as web_log:
        app = subprocess.Popen(args_app, cwd=directory, env=env, stdout=log, stderr=subprocess.STDOUT)
        root = psutil.Process(app.pid)
        try:
            while time.perf_counter() - start < seconds + 90:
                elapsed = time.perf_counter() - start
                if app.poll() is not None:
                    if app.returncode:
                        raise RuntimeError(f"App exited {app.returncode}, see {directory}")
                    break
                if first_window is None and visible_window(app.pid):
                    first_window = elapsed
                if engine == "tauri" and driver is None and first_window is not None:
                    try:
                        with urllib.request.urlopen(f"http://127.0.0.1:{endpoint}/json/version", timeout=.25):
                            pass
                    except OSError:
                        pass
                    else:
                        driver = subprocess.Popen(["node", HERE / "webview.mjs", "run", run_dir / "dist",
                                                   case_path, str(endpoint), ready], stdout=web_log, stderr=subprocess.STDOUT)
                if driver and driver.poll() is not None:
                    if driver.returncode:
                        raise RuntimeError(f"WebView2 driver exited {driver.returncode}, see {directory}")
                    break
                if active is None and ready.exists():
                    active = time.perf_counter()
                if engine == "gpui" and mode in ("idle", "workspace") and first_window is not None:
                    active = start + first_window + 3
                    if mode == "idle" and time.perf_counter() - active >= seconds:
                        break
                if mode == "workspace" and active and time.perf_counter() - active >= seconds and accessibility is None:
                    raw = subprocess.check_output(["powershell", "-NoProfile", "-File", HERE.parents[2] / "scripts" / "check-desktop-accessibility.ps1",
                                                   "-AppProcessId", str(app.pid)], timeout=25)
                    accessibility = json.loads(raw)
                    if engine == "gpui":
                        break
                    ready.with_suffix(".done").touch()
                try:
                    samples.append(sample(root, cpu))
                    if gpu and (not gpu_samples or time.perf_counter() - gpu_samples[-1][0] >= 1):
                        gpu_samples.append((time.perf_counter(), gpu.sample(set(cpu))))
                except psutil.NoSuchProcess:
                    pass
                time.sleep(.09)
            else:
                raise TimeoutError(f"Trial exceeded its deadline: {directory}")
        finally:
            if driver and driver.poll() is None:
                driver.kill()
                driver.wait()
            stop(root)
            app.wait()
            if gpu:
                gpu.close()
    if first_window is None:
        raise RuntimeError(f"No visible window: {directory}")
    if engine == "tauri":
        lines = (directory / "webview.log").read_text().splitlines()
        metrics = json.loads(lines[-1])
    elif mode == "stream":
        metrics = json.loads((directory / "bench-last.json").read_text())
        if abs(metrics["tokens"] - rate * seconds) > rate * .05 or metrics["live_chars"] == 0:
            raise RuntimeError(f"Incomplete stream: {directory}")
    elif mode == "scroll":
        import re
        line = (directory / "app.log").read_text(errors="replace")
        match = re.search(r"SCROLLBENCH draw_ms n=(\d+) mean=([\d.]+) p50=([\d.]+) p95=([\d.]+) p99=([\d.]+)", line)
        if not match:
            raise RuntimeError(f"No scroll result: {directory}")
        metrics = dict(zip(("frames", "meanDrawMs", "p50DrawMs", "p95DrawMs", "p99DrawMs"), map(float, match.groups())))
    else:
        metrics = {}
    if active is None:
        active = samples[-1][0] - seconds
    # Use the interior of the timed phase. Process startup and window teardown are separate.
    block = [row for row in samples if active + .5 <= row[0] <= active + seconds - .5]
    if len(block) < max(5, seconds * 2) or block[-1][0] - block[0][0] < seconds - 2:
        raise RuntimeError(f"Incomplete process samples: {directory}")
    gpu_metrics = None
    if gpu:
        readings = [values for at, values in gpu_samples if active + 1.5 <= at <= active + seconds - .5 and values]
        if len(readings) < 3:
            raise RuntimeError(f"Insufficient GPU process counters: {directory}")
        gpu_metrics = {key: round(statistics.median(row[key] for row in readings), 2) for key in readings[0]}
        gpu_metrics["samples"] = len(readings)
    return dict(engine=engine, mode=mode, history=n, rate=rate, repetition=number,
                firstWindowMs=round(first_window*1000), sampleCount=len(block),
                cpuCores=round((block[-1][1]-block[0][1])/(block[-1][0]-block[0][0]),3),
                privateMiB=round(statistics.median(row[2] for row in block),1),
                peakPrivateMiB=round(max(row[2] for row in block),1),
                workingSetMiB=round(statistics.median(row[3] for row in block),1),
                processes=statistics.median(row[4] for row in block), renderer=metrics, accessibility=accessibility,
                gpu=gpu_metrics)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tauri", type=Path, required=True)
    parser.add_argument("--gpui", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True, help="New directory for this run")
    parser.add_argument("--repetitions", type=int, default=3)
    parser.add_argument("--seconds", type=int, default=15)
    parser.add_argument("--only", choices=["idle","stream","scroll"])
    parser.add_argument("--shell", action="store_true", help="Compare empty workspaces and inspect their Windows accessibility trees")
    parser.add_argument("--stress", action="store_true", help="Explicitly open 5,000 rendered messages in each frontend")
    parser.add_argument("--gpu", action="store_true", help="Sample Windows GPU counters for 20 and 5,000 messages (requires pywin32)")
    args = parser.parse_args()
    args.gpui = args.gpui.resolve(strict=True)
    args.tauri = args.tauri.resolve(strict=True)
    if args.seconds < 5 or args.repetitions < 1:
        parser.error("Use at least five seconds and one repetition")
    if sum(bool(flag) for flag in [args.only, args.shell, args.stress, args.gpu]) > 1:
        parser.error("Choose one of --shell, --stress, --gpu, or --only")
    if (args.stress or args.gpu) and psutil.virtual_memory().available < 8 * 2**30:
        parser.error("The 5,000-row DOM stress run needs 8 GiB of available physical memory")
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    subprocess.run(["rustc", "--edition", "2024", "-O", HERE/"seed_export.rs", "-o", out/"seed.exe"], check=True)
    subprocess.run(["node", HERE/"webview.mjs", "build", out/"dist"], cwd=ROOT, check=True)
    cases = CASES
    if args.shell:
        cases = [("workspace", 0, 0)]
    elif args.stress:
        cases = [("stream", 5000, 75)]
    elif args.gpu:
        cases = [("stream", 20, 75), ("stream", 5000, 75)]
    for rep in range(1, args.repetitions + 1):
        for i, case in enumerate(cases):
            if args.only and args.only != case[0]:
                continue
            engines = ("gpui", "tauri") if (rep+i) % 2 == 0 else ("tauri", "gpui")
            for engine in engines:
                row = trial(args, out, engine, case, rep)
                with (out/"results.jsonl").open("a", encoding="utf-8") as results:
                    print(json.dumps(row), file=results)
                print(rep, engine, case, row["privateMiB"], "MiB", flush=True)


if __name__ == "__main__":
    main()
