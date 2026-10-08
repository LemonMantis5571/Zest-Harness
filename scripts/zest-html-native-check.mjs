// Opt-in proof against the actual debug Tauri/WebView2 host, without a provider.
// PowerShell: $env:ZEST_HTML_NATIVE_CHECK='1'; node scripts/zest-html-native-check.mjs
// Build zest-desktop separately before opting in. No browser is installed here.
// Host mode is CDP-free. Set ZEST_HTML_NATIVE_CHECK_DRIVER=cdp for real Playwright
// pointer interaction/screenshots when this runtime exposes its private endpoint.

import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { access, lstat, mkdir, mkdtemp, readFile, realpath, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { setTimeout as delay } from "node:timers/promises";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const executable = path.join(root, "target", "debug", "zest-desktop.exe");
const hostUrl = "http://zest-check.localhost/index.html";
const stepTimeout = 8_000;
const runTimeout = 90_000;
const tempPrefix = "zest-html-native-check-";

class CheckFailure extends Error {}

function requireCheck(condition, message) {
  if (!condition) throw new CheckFailure(message);
}

async function bounded(operation, message, timeout = stepTimeout) {
  let timer;
  try {
    return await Promise.race([
      operation,
      new Promise((_, reject) => {
        timer = setTimeout(() => reject(new CheckFailure(message)), timeout);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

function cleanEnvironment() {
  return Object.fromEntries(Object.entries(process.env).filter(([name]) =>
    !/key|token|password|secret|credential|authorization|cookie|(^|_)(auth|pwd|pass)(_|$)/i.test(name)
    && !/^(WEBVIEW2_|TAURI_|ZEST_|RUST_LOG$|RUST_BACKTRACE$|DEBUG$|PWDEBUG$|NODE_OPTIONS$)/i.test(name)
    && !/^(HTTPS?_PROXY|ALL_PROXY|NO_PROXY)$/i.test(name),
  ));
}

async function listen(server, port = 0) {
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(port, "127.0.0.1", () => {
      server.off("error", reject);
      resolve();
    });
  });
  return server.address().port;
}

async function closeServer(server) {
  if (!server?.listening) return;
  const closed = new Promise((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
  server.closeAllConnections?.();
  await bounded(closed, "The loopback server did not close", 3_000);
}

// Only process IDs are captured. In particular, native stderr can contain Tauri's
// genuine invoke key after a forged message, so every spawned stderr is ignored.
async function quietProcess(command, args, env, capture = false) {
  const child = spawn(command, args, {
    windowsHide: true,
    stdio: ["ignore", capture ? "pipe" : "ignore", "ignore"],
    env,
  });
  let output = "";
  child.stdout?.on("data", (chunk) => {
    if (output.length < 64 * 1024) output += chunk.toString();
  });
  const completed = new Promise((resolve, reject) => {
    child.once("error", reject);
    child.once("close", (code) => resolve({ code, output }));
  });
  try {
    return await bounded(completed, "A cleanup process did not finish", 6_000);
  } finally {
    if (child.exitCode === null && child.signalCode === null) child.kill();
  }
}

async function profileProcesses(profile, env) {
  const script = String.raw`
    $ErrorActionPreference = 'Stop'
    $nativeCheckPaths = @([IO.Path]::GetFullPath($env:ZEST_HTML_CHECK_PROFILE), [IO.Path]::GetFullPath([IO.Path]::Combine($env:ZEST_HTML_CHECK_PROFILE, 'EBWebView')))
    $nativeCheckPattern = '(?i)(?:^|\s)(?:"--user-data-dir=([^"]+)"|--user-data-dir="([^"]+)"|--user-data-dir=([^\s]+))'
    $nativeCheckIds = @(Get-CimInstance Win32_Process -Filter "Name = 'msedgewebview2.exe'" |
      Where-Object {
        $nativeCheckMatch = [regex]::Match($_.CommandLine, $nativeCheckPattern)
        if (-not $nativeCheckMatch.Success) { return $false }
        $nativeCheckValue = @($nativeCheckMatch.Groups | Select-Object -Skip 1 | Where-Object Success | ForEach-Object Value)[0]
        $nativeCheckActual = [IO.Path]::GetFullPath($nativeCheckValue)
        return @($nativeCheckPaths | Where-Object { [string]::Equals($_, $nativeCheckActual, [StringComparison]::OrdinalIgnoreCase) }).Count -gt 0
      } |
      ForEach-Object { [int]$_.ProcessId })
    ConvertTo-Json -Compress -InputObject $nativeCheckIds
  `;
  const result = await quietProcess("powershell.exe", [
    "-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", script,
  ], { ...env, ZEST_HTML_CHECK_PROFILE: profile }, true);
  requireCheck(result.code === 0, "Could not verify that the private WebView2 process tree was reaped");
  let ids;
  try { ids = JSON.parse(result.output); } catch { throw new CheckFailure("Could not read private WebView2 process IDs"); }
  requireCheck(Array.isArray(ids) && ids.every((id) => Number.isSafeInteger(id) && id > 0), "Invalid cleanup process IDs");
  return ids;
}

async function reapNative(child, closed, profile, env) {
  if (child?.pid && child.exitCode === null && child.signalCode === null) {
    await quietProcess("taskkill.exe", ["/PID", String(child.pid), "/T", "/F"], env);
  }
  if (closed) await bounded(closed, "The native check process did not exit", 5_000);
  // Also handles WebView2 children whose native parent exited before cleanup.
  for (let pass = 0; pass < 3; pass += 1) {
    const ids = await profileProcesses(profile, env);
    if (ids.length === 0) return;
    for (const id of ids) await quietProcess("taskkill.exe", ["/PID", String(id), "/T", "/F"], env);
  }
  requireCheck((await profileProcesses(profile, env)).length === 0, "A private WebView2 process survived cleanup");
}

async function removeRuntime(directory) {
  if (!directory) return;
  const absolute = path.resolve(directory);
  const parent = path.resolve(tmpdir());
  requireCheck(path.dirname(absolute) === parent && path.basename(absolute).startsWith(tempPrefix)
    && path.basename(absolute).length > tempPrefix.length, "Refused an unsafe temporary-directory cleanup");
  const info = await lstat(absolute);
  requireCheck(info.isDirectory() && !info.isSymbolicLink(), "Refused a replaced temporary directory");
  requireCheck(path.dirname(await realpath(absolute)) === await realpath(parent), "Temporary directory escaped its parent");
  await rm(absolute, { recursive: true, force: false, maxRetries: 8, retryDelay: 150 });
}

async function connectNative(chromium, profile, child, state) {
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    requireCheck(!state.spawnFailed && child.exitCode === null && child.signalCode === null,
      "The debug native host exited before CDP was ready");
    let ready = false;
    let endpoint;
    try {
      const discovered = await readFile(path.join(profile, "EBWebView", "DevToolsActivePort"), "utf8");
      requireCheck(discovered.length <= 1024, "Invalid private CDP discovery file");
      const [port, socketPath] = discovered.trim().split(/\r?\n/);
      requireCheck(/^\d{1,5}$/.test(port) && Number(port) > 0 && Number(port) <= 65535
        && /^\/devtools\/browser\/[A-Za-z0-9-]+$/.test(socketPath), "Invalid private CDP discovery endpoint");
      endpoint = `http://127.0.0.1:${port}`;
      const response = await fetch(`${endpoint}/json/version`, { signal: AbortSignal.timeout(750) });
      if (response.ok) {
        const version = await response.json();
        const socket = new URL(version.webSocketDebuggerUrl);
        requireCheck(socket.protocol === "ws:" && ["127.0.0.1", "localhost"].includes(socket.hostname)
          && socket.port === new URL(endpoint).port && socket.pathname === socketPath
          && !socket.username && !socket.password && !socket.search && !socket.hash,
        "CDP did not advertise the discovered private loopback endpoint");
        ready = true;
      }
    } catch (error) {
      if (error instanceof CheckFailure) throw error;
    }
    if (ready) {
      try { return await chromium.connectOverCDP(endpoint, { timeout: 5_000 }); }
      catch (error) {
        const method = /Protocol error \(([A-Za-z]+\.[A-Za-z]+)\)/.exec(error.message)?.[1];
        throw new CheckFailure(`CDP is ready, but Playwright attachment failed${method ? ` at ${method}` : ""}; raw diagnostics are suppressed`);
      }
    }
    await delay(100);
  }
  throw new CheckFailure("CDP did not publish its owned EBWebView discovery endpoint within 30 seconds");
}

async function invoke(page, command, payload = {}) {
  return bounded(page.evaluate(({ command, payload }) =>
    window.__TAURI_INTERNALS__.invoke(command, payload), { command, payload }),
  `Native command ${command} did not finish`);
}

// This function is serialized into the untrusted document's own inline script.
// CDP reads its DOM results; it does not inject the attacks into an exempt world.
function artifactProbes(config) {
  const output = document.getElementById("results");
  const results = { phase: "ready", violations: [], attempted: [], probes: {} };
  const write = () => { output.textContent = JSON.stringify(results); };
  const kind = (error) => error?.name || "denied";
  const sync = (name, action) => {
    results.attempted.push(name);
    try { results.probes[name] = action(); } catch (error) { results.probes[name] = kind(error); }
  };
  const settle = async (name, action) => {
    results.attempted.push(name);
    let timer;
    try {
      results.probes[name] = await Promise.race([
        Promise.resolve().then(action).then(() => "resolved", () => "denied"),
        new Promise((resolve) => { timer = setTimeout(() => resolve("pending"), 1_500); }),
      ]);
    } finally { clearTimeout(timer); }
  };
  document.addEventListener("securitypolicyviolation", (event) => {
    if (!results.violations.includes(event.effectiveDirective)) results.violations.push(event.effectiveDirective);
    write();
  });
  function rtc() {
    return ["RTCPeerConnection", "webkitRTCPeerConnection"].every((name) => {
      const before = Object.getOwnPropertyDescriptor(window, name);
      try { window[name] = function Replacement() {}; } catch {}
      try { Object.defineProperty(window, name, { value: function Replacement() {} }); } catch {}
      const after = Object.getOwnPropertyDescriptor(window, name);
      let constructionDenied = false;
      try { new window[name](); } catch { constructionDenied = true; }
      return before?.value === undefined && before.writable === false && before.configurable === false
        && after?.value === undefined && after.writable === false && after.configurable === false
        && constructionDenied;
    });
  }
  results.rtcBefore = rtc();
  document.getElementById("increment").onclick = () => {
    const counter = document.getElementById("counter");
    counter.textContent = String(Number(counter.textContent) + 1);
  };
  document.getElementById("run-probes").onclick = async () => {
    results.userActivationAtProbe = navigator.userActivation.isActive;
    results.phase = "running";
    write();
    sync("parentRead", () => { void parent.document.body; return "accessible"; });
    sync("parentWrite", () => { parent.document.getElementById("host").textContent = "compromised"; return "accessible"; });
    sync("parentInvoke", () => { void parent.__TAURI_INTERNALS__.invoke; return "accessible"; });
    sync("sameOriginSibling", () => { void parent.frames[1].document.body; return "accessible"; });
    sync("nestedRealm", () => {
      const nested = document.createElement("iframe");
      document.body.append(nested);
      try {
        const realm = nested.contentWindow;
        void realm.document.body;
        for (const name of ["RTCPeerConnection", "webkitRTCPeerConnection"]) {
          if (typeof realm[name] === "function") {
            const connection = new realm[name]();
            connection.close();
            return "rtcRecovered";
          }
        }
        return "rtcBlocked";
      } finally { nested.remove(); }
    });
    sync("storage", () => { localStorage.setItem("native-check", "bad"); return "accessible"; });
    sync("popup", () => window.open(`${config.canary}/popup`, "_blank") === null ? "blocked" : "opened");
    sync("topNavigation", () => { top.location.href = `${config.canary}/top`; return "accepted"; });
    sync("parentNavigation", () => { parent.location.href = `${config.canary}/parent`; return "accepted"; });
    sync("sharedWorker", () => {
      if (typeof SharedWorker !== "function") return "absent";
      const url = URL.createObjectURL(new Blob(["onconnect=()=>{}"], { type: "text/javascript" }));
      try { const worker = new SharedWorker(url); worker.port.close(); return "created"; } finally { URL.revokeObjectURL(url); }
    });
    const form = document.createElement("form");
    form.action = `${config.canary}/form`;
    form.method = "post";
    document.body.append(form);
    sync("form", () => { form.requestSubmit(); return "attempted"; });
    sync("beacon", () => navigator.sendBeacon(`${config.canary}/beacon`, "probe") ? "accepted" : "blocked");

    const forged = {
      cmd: "prepare_html_view", callback: 2147483001, error: 2147483002,
      payload: { document: { title: "Forged native call", html: "<p>forged</p>" } },
      options: { headers: {}, customProtocolIpcBlocked: true },
      __TAURI_INVOKE_KEY__: "zest-native-check-deliberately-wrong-key",
    };
    sync("foreignMessages", () => {
      parent.postMessage({ kind: "zest-html-check-ordinary", action: "prepare_html_view", document: forged.payload.document }, "*");
      parent.postMessage({ kind: "zest-html-check-forged", ...forged }, "*");
      parent.postMessage(JSON.stringify(forged), "*");
      return "sent";
    });
    sync("rawNativeInvoke", () => {
      if (typeof window.chrome?.webview?.postMessage === "function") {
        window.chrome.webview.postMessage(JSON.stringify(forged));
        return "sent";
      }
      if (typeof window.ipc?.postMessage === "function") {
        window.ipc.postMessage(JSON.stringify(forged));
        return "sent";
      }
      return "absent";
    });

    const network = (suffix) => `${config.canary}/${suffix}`;
    const xhr = (url) => new Promise((resolve, reject) => {
      const request = new XMLHttpRequest();
      request.onload = resolve;
      request.onerror = reject;
      request.ontimeout = reject;
      request.timeout = 1_000;
      request.open("GET", url);
      request.send();
    });
    const resource = (tag, url, attributes = {}) => new Promise((resolve, reject) => {
      const element = document.createElement(tag);
      Object.assign(element, attributes);
      if (tag === "object") element.style.cssText = "display:block;width:1px;height:1px";
      element.onload = resolve;
      element.onerror = reject;
      if (tag === "link") element.href = url;
      else if (tag === "object") element.data = url;
      else element.src = url;
      document.body.append(element);
    });
    const tasks = [
      settle("worker", () => new Promise((resolve, reject) => {
        const url = URL.createObjectURL(new Blob([`postMessage('native-worker-executed');fetch(${JSON.stringify(config.canary + "/worker")});`], { type: "text/javascript" }));
        let worker;
        const finish = (done) => { worker?.terminate(); URL.revokeObjectURL(url); done(); };
        try {
          worker = new Worker(url);
          worker.onmessage = () => finish(resolve);
          worker.onerror = () => finish(reject);
        } catch { finish(reject); }
      })),
      settle("fetch", () => fetch(network("fetch"))),
      settle("xhr", () => xhr(network("xhr"))),
      settle("websocket", () => new Promise((resolve, reject) => {
        const socket = new WebSocket(network("websocket").replace("http:", "ws:"));
        socket.onopen = () => { socket.close(); resolve(); };
        socket.onerror = reject;
      })),
      settle("eventsource", () => new Promise((resolve, reject) => {
        const source = new EventSource(network("eventsource"));
        source.onopen = () => { source.close(); resolve(); };
        source.onerror = () => { source.close(); reject(); };
      })),
      settle("image", () => resource("img", network("image"))),
      settle("script", () => resource("script", network("script"))),
      settle("stylesheet", () => resource("link", network("stylesheet"), { rel: "stylesheet" })),
      settle("frame", () => resource("iframe", network("frame"))),
      settle("object", () => resource("object", network("object"))),
      settle("localFetch", () => fetch(config.file)),
      settle("localXhr", () => xhr(config.file)),
      settle("localFrame", () => resource("iframe", config.file)),
      settle("localScript", () => resource("script", config.file)),
      settle("localImage", () => resource("img", config.file)),
      settle("ipcFetch", () => fetch("http://ipc.localhost/prepare_html_view", {
        method: "POST",
        headers: { "Content-Type": "application/json", "Tauri-Callback": "2147483001", "Tauri-Error": "2147483002", "Tauri-Invoke-Key": forged.__TAURI_INVOKE_KEY__ },
        body: JSON.stringify(forged.payload),
      })),
    ];
    if (typeof window.__TAURI_INTERNALS__?.invoke === "function") {
      tasks.push(settle("bridgeInvoke", () => window.__TAURI_INTERNALS__.invoke("prepare_html_view", forged.payload)));
    } else results.probes.bridgeInvoke = "absent";
    await Promise.all(tasks);
    results.rtcAfter = rtc();
    results.canaryExecuted = window.__zestNativeCanaryExecuted === true;
    results.counterValue = Number(document.getElementById("counter").textContent);
    results.documentStillProtocol = /^http:\/\/zest-html\.localhost\/[0-9a-f]{64}$/.test(location.href);
    results.phase = "done";
    write();
    if (config.selfCheck) parent.postMessage({ kind: "zest-html-check-result", results }, "*");
  };
  write();
  if (config.selfCheck) {
    // Runs in the document's own ordinary inline realm, never a CDP world.
    // HTMLElement.click() is deliberately not described as user activation.
    window.addEventListener("load", () => setTimeout(() => {
      document.getElementById("increment").click();
      document.getElementById("increment").click();
      document.getElementById("run-probes").click();
    }, 100), { once: true });
  }
}

function documentSource(config) {
  const json = JSON.stringify(config).replaceAll("<", "\\u003c");
  return `<meta http-equiv="Content-Security-Policy" content="default-src * data: blob: 'unsafe-inline' 'unsafe-eval'">
<style>body{font:16px system-ui;margin:24px;background:#f8fafc;color:#172033}button{padding:10px 16px;margin:8px 12px 8px 0}pre{white-space:pre-wrap;overflow-wrap:anywhere}iframe,img,object{display:none}</style>
<h1>Native HTML confinement check</h1><p>Inline counter: <output id="counter">0</output></p>
<button id="increment">Increment</button><button id="run-probes">Run confinement probes</button>
<pre id="results"></pre><script>(${artifactProbes.toString()})(${json});</script>`;
}

// Main-frame initialization only. Foreign messages never become commands; the
// one debug-only result envelope is accepted from this exact opaque iframe.
function nativeHostDriver(html) {
  if (window.top !== window || location.href !== "http://zest-check.localhost/index.html") return;
  const start = async () => {
    const checks = Object.fromEntries([
      "empty_registry", "prepared_two_views", "sandbox_attributes", "foreign_messages",
      "artifact_dispatch_ignored", "wrong_key_dispatch_ignored", "release_idempotent", "main_host_unchanged",
    ].map((name) => [name, false]));
    const tokens = new Set();
    let results = null;
    let step = "initialization";
    let failedStep = null;
    const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
    const limit = async (operation, ms = 8_000) => {
      let timer;
      try {
        return await Promise.race([operation, new Promise((_, reject) => {
          timer = setTimeout(() => reject(new Error("timeout")), ms);
        })]);
      } finally { clearTimeout(timer); }
    };
    const invoke = (command, payload = {}) => limit(window.__TAURI_INTERNALS__.invoke(command, payload));
    const assert = (condition) => { if (!condition) throw new Error("native-check"); };
    const validView = (view) => view && /^[0-9a-f]{64}$/.test(view.token)
      && view.url === `http://zest-html.localhost/${view.token}`;
    const load = (frame, url) => limit(new Promise((resolve) => {
      frame.addEventListener("load", resolve, { once: true });
      frame.src = url;
      document.body.append(frame);
    }));
    try {
      assert(typeof window.__TAURI_INTERNALS__?.invoke === "function");
      const original = { host: document.getElementById("host").outerHTML, title: document.title, url: location.href };
      checks.empty_registry = await invoke("html_check_count") === 0;
      assert(checks.empty_registry);
      step = "prepare";
      const prepared = await invoke("prepare_html_view", { document: { title: "Native HTML confinement check", html } });
      assert(validView(prepared));
      tokens.add(prepared.token);
      const sibling = await invoke("prepare_html_view", { document: { title: "Same origin isolation canary", html: "<p>Synthetic sibling canary</p>" } });
      assert(validView(sibling) && sibling.token !== prepared.token);
      tokens.add(sibling.token);
      checks.prepared_two_views = await invoke("html_check_count") === 2;
      assert(checks.prepared_two_views);
      const frame = document.createElement("iframe");
      frame.id = "artifact";
      frame.setAttribute("sandbox", "allow-scripts");
      frame.style.cssText = "width:100%;height:600px;border:0";
      const other = document.createElement("iframe");
      other.setAttribute("sandbox", "allow-scripts");
      other.style.display = "none";
      const messages = { ordinary: 0, forged: 0, string: 0 };
      const result = new Promise((resolve) => {
        window.addEventListener("message", (event) => {
          if (event.source !== frame.contentWindow || event.origin !== "null") return;
          if (event.data?.kind === "zest-html-check-ordinary") messages.ordinary += 1;
          if (event.data?.kind === "zest-html-check-forged") messages.forged += 1;
          if (typeof event.data === "string") messages.string += 1;
          if (event.data?.kind === "zest-html-check-result" && JSON.stringify(event.data).length <= 8192) resolve(event.data.results);
        });
      });
      step = "artifact";
      await Promise.all([load(frame, prepared.url), load(other, sibling.url)]);
      checks.sandbox_attributes = frame.getAttribute("sandbox") === "allow-scripts" && other.getAttribute("sandbox") === "allow-scripts";
      results = await limit(result);
      assert(results?.phase === "done");
      checks.foreign_messages = messages.ordinary === 1 && messages.forged === 1 && messages.string === 1;
      checks.artifact_dispatch_ignored = await invoke("html_check_count") === 2;
      step = "dispatch";
      const forged = JSON.stringify({
        cmd: "prepare_html_view", callback: 2147483001, error: 2147483002,
        payload: { document: { title: "Forged host dispatch", html: "<p>forged</p>" } },
        options: { headers: {}, customProtocolIpcBlocked: true },
        __TAURI_INVOKE_KEY__: "zest-native-check-deliberately-wrong-key",
      });
      if (typeof window.chrome?.webview?.postMessage === "function") window.chrome.webview.postMessage(forged);
      else if (typeof window.ipc?.postMessage === "function") window.ipc.postMessage(forged);
      else throw new Error("transport");
      checks.wrong_key_dispatch_ignored = true;
      for (let pass = 0; pass < 8; pass += 1) {
        await pause(100);
        if (await invoke("html_check_count") !== 2) checks.wrong_key_dispatch_ignored = false;
      }
      step = "release";
      await invoke("release_html_view", { token: prepared.token });
      await invoke("release_html_view", { token: prepared.token });
      tokens.delete(prepared.token);
      await invoke("release_html_view", { token: sibling.token });
      tokens.delete(sibling.token);
      checks.release_idempotent = await invoke("html_check_count") === 0;
      frame.remove();
      const revoked = document.createElement("iframe");
      revoked.setAttribute("sandbox", "allow-scripts");
      await load(revoked, prepared.url);
      step = "integrity";
      checks.main_host_unchanged = location.href === original.url && document.title === original.title
        && document.getElementById("host").outerHTML === original.host;
    } catch { failedStep = step; }
    finally {
      for (const token of tokens) {
        try { await invoke("release_html_view", { token }); } catch { failedStep ||= "release"; }
      }
      try { await invoke("html_check_report", { report: { checks, results, failedStep } }); } catch {}
    }
  };
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", start, { once: true });
  else void start();
}

function validateProbes(result, formsDenied = 0, nativeSelfCheck = false, canaryRequests = -1, guardDenied = 0, preparesSeen = -1) {
  requireCheck(result?.phase === "done" && result.counterValue === 2 && result.documentStillProtocol,
    "The real protocol HTML did not complete its inline counter and probes without navigation");
  for (const name of ["parentRead", "parentWrite", "parentInvoke", "sameOriginSibling", "storage", "topNavigation", "parentNavigation"]) {
    requireCheck(result.probes[name] === "SecurityError", `The native HTML document did not deny ${name}`);
  }
  requireCheck(["absent", "denied"].includes(result.probes.bridgeInvoke)
    || (result.probes.bridgeInvoke === "pending" && (guardDenied > 0 || preparesSeen === 2)),
  "The artifact's native invoke bridge was neither absent nor observably prevented from native dispatch");
  requireCheck(result.probes.popup === "blocked", "The HTML document opened a popup");
  requireCheck(result.probes.worker === "denied" && ["absent", "SecurityError"].includes(result.probes.sharedWorker), "The HTML document executed a worker");
  requireCheck(result.probes.form === "attempted" && (formsDenied > 0 || result.violations.includes("form-action")
    || (nativeSelfCheck && canaryRequests === 0 && result.documentStillProtocol)), "No actual form-submission denial was observed");
  requireCheck(result.rtcBefore && result.rtcAfter, "The document could restore an RTC constructor");
  requireCheck(result.attempted.includes("nestedRealm") && ["SecurityError", "rtcBlocked"].includes(result.probes.nestedRealm), "An initial about:blank nested realm recovered a working RTC constructor");
  for (const name of ["fetch", "xhr", "websocket", "eventsource", "image", "script", "stylesheet", "localFetch", "localXhr", "localScript", "localImage", "ipcFetch"]) {
    requireCheck(result.attempted.includes(name) && result.probes[name] === "denied", `The actual ${name} probe was not denied`);
  }
  for (const name of ["frame", "object", "localFrame"]) requireCheck(result.attempted.includes(name), `The HTML document did not attempt ${name}`);
  requireCheck(["blocked", "SecurityError"].includes(result.probes.beacon)
    || (result.probes.beacon === "accepted" && canaryRequests === 0 && result.violations.includes("connect-src")),
  "A beacon was not observably confined by CSP and the independent canary");
  for (const directive of ["connect-src", "img-src", "frame-src", "object-src", "worker-src"]) requireCheck(result.violations.includes(directive), `The browser did not report the expected ${directive} violation`);
  requireCheck(result.violations.some((name) => name.startsWith("script-src")), "No actual external-script CSP denial was observed");
  requireCheck(result.violations.some((name) => name.startsWith("style-src")), "No actual external-stylesheet CSP denial was observed");
  requireCheck(!result.canaryExecuted, "A network or local-file canary executed");
}

function checkCsp(header) {
  const directives = new Map(header.split(";").map((part) => {
    const [name, ...values] = part.trim().split(/\s+/);
    return [name.toLowerCase(), values];
  }));
  for (const directive of ["default-src", "connect-src", "frame-src", "object-src", "worker-src", "base-uri", "form-action"]) {
    requireCheck(JSON.stringify(directives.get(directive)) === '["\'none\'"]', `Native HTML CSP must deny ${directive}`);
  }
  requireCheck(JSON.stringify(directives.get("sandbox")) === '["allow-scripts"]', "Native HTML CSP relaxed the frame sandbox");
  requireCheck(JSON.stringify(directives.get("script-src")) === '["\'unsafe-inline\'"]', "Native HTML CSP changed inline script confinement");
  requireCheck(JSON.stringify(directives.get("style-src")) === '["\'unsafe-inline\'"]', "Native HTML CSP changed inline style confinement");
}

async function main() {
  if (process.env.ZEST_HTML_NATIVE_CHECK !== "1") {
    return { ok: true, skipped: true, paidProviderCalls: 0, reason: "Set ZEST_HTML_NATIVE_CHECK=1 to run the Windows native check" };
  }
  requireCheck(process.platform === "win32", "The native HTML check requires Windows and WebView2");
  requireCheck(process.argv.length === 2, "This check takes no arguments; build target/debug/zest-desktop.exe separately");
  const driver = process.env.ZEST_HTML_NATIVE_CHECK_DRIVER || "host";
  requireCheck(["host", "cdp"].includes(driver), "ZEST_HTML_NATIVE_CHECK_DRIVER must be host or cdp");
  try { await access(executable); } catch { throw new CheckFailure("Build target/debug/zest-desktop.exe with the debug HTML check before opting in"); }
  let chromium;
  try {
    // Playwright's optional protocol logger must not dump native payloads.
    delete process.env.DEBUG;
    delete process.env.PWDEBUG;
    ({ chromium } = createRequire(path.join(root, "crates", "desktop", "ui", "package.json"))("@playwright/test"));
  } catch { throw new CheckFailure("Install the repository UI dependencies before running the native check"); }

  const report = { ok: false, skipped: false, driver, paidProviderCalls: 0, checks: [], diagnostics: { consoleErrors: 0, pageErrors: 0, failedRequests: 0 } };
  let stage = "setup";
  let runtime;
  let artifacts;
  let profile;
  let server;
  let child;
  let childClosed;
  let browser;
  let page;
  let session;
  let env = cleanEnvironment();
  const cancellation = new AbortController();
  const interrupted = () => cancellation.abort();
  process.on("SIGINT", interrupted);
  process.on("SIGTERM", interrupted);
  const tokens = new Set();
  const completed = (name, details = {}) => { report.checks.push({ name, ok: true, ...details }); };
  const cleanupErrors = [];

  try {
    runtime = await mkdtemp(path.join(tmpdir(), tempPrefix));
    profile = path.join(runtime, "webview2");
    const runId = randomUUID();
    artifacts = path.join(root, "target", "html-native-check", runId);
    await mkdir(artifacts, { recursive: true });
    for (const directory of [profile, "home", "appdata", "localappdata", "temp"].map((name) => path.isAbsolute(name) ? name : path.join(runtime, name))) {
      await mkdir(directory, { recursive: true });
    }
    const file = path.join(runtime, "local-canary.html");
    await writeFile(file, "<!doctype html><title>Synthetic local canary</title><script>window.__zestNativeCanaryExecuted=true</script><p>Local canary must remain unread</p>", { flag: "wx" });
    const fileUrl = pathToFileURL(file).href;
    const hits = [];
    const canaryPrefix = `/canary/${runId}/`;
    const recordHit = (request) => {
      if (request.url?.startsWith(canaryPrefix)) hits.push(request.url.slice(canaryPrefix.length).split("?")[0].slice(0, 80));
    };
    server = createServer((request, response) => {
      recordHit(request);
      response.writeHead(200, { "Content-Type": "text/javascript", "Cache-Control": "no-store", "Access-Control-Allow-Origin": "*" });
      response.end("window.__zestNativeCanaryExecuted=true;");
    });
    server.on("upgrade", (request, socket) => { recordHit(request); socket.destroy(); });
    const canaryPort = await listen(server);
    const calibration = await fetch(`http://127.0.0.1:${canaryPort}/control`, { signal: AbortSignal.timeout(2_000) });
    requireCheck(calibration.ok && (await calibration.text()).includes("__zestNativeCanaryExecuted"), "The loopback canary failed its positive control");
    requireCheck(hits.length === 0, "The loopback canary recorded an unexpected initial request");
    completed("loopback_canary_positive_control");

    const debugPort = 0;
    if (driver === "host") {
      const html = documentSource({ canary: `http://127.0.0.1:${canaryPort}/canary/${runId}`, file: fileUrl, selfCheck: true });
      await writeFile(path.join(runtime, "native-driver.js"), `(${nativeHostDriver.toString()})(${JSON.stringify(html).replaceAll("<", "\\u003c")});`, { flag: "wx" });
      report.limitations = [
        "Native inline buttons use HTMLElement.click() without user activation; activation-sensitive cases also require browser fixture tests.",
        "CDP-free mode has no compositor screenshot, console observer, or local-file response observer. Local-file denial is checked by real inline resource attempts and CSP events.",
      ];
      report.diagnostics.consoleErrors = null;
      report.diagnostics.pageErrors = null;
      report.diagnostics.failedRequests = null;
    }
    env = {
      ...env, HOME: path.join(runtime, "home"), USERPROFILE: path.join(runtime, "home"),
      APPDATA: path.join(runtime, "appdata"), LOCALAPPDATA: path.join(runtime, "localappdata"),
      TEMP: path.join(runtime, "temp"), TMP: path.join(runtime, "temp"),
      WEBVIEW2_USER_DATA_FOLDER: profile,
      ZEST_HTML_CHECK_DEBUG_PORT: String(debugPort),
      ZEST_HTML_CHECK_SELF_DRIVER: driver === "host" ? "1" : "0",
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-address=127.0.0.1 --remote-debugging-port=${debugPort} --no-first-run --no-default-browser-check`,
    };
    stage = "native startup";
    const state = { spawnFailed: false };
    child = spawn(executable, ["--html-native-check"], { cwd: runtime, env, windowsHide: true, stdio: "ignore" });
    childClosed = new Promise((resolve) => { child.once("close", resolve); });
    child.once("error", () => { state.spawnFailed = true; });

    const run = async () => {
      requireCheck(!cancellation.signal.aborted, "The native check was interrupted");
      if (driver === "host") {
        stage = "native host harness";
        let observed;
        const deadline = Date.now() + 35_000;
        while (!observed && Date.now() < deadline) {
          requireCheck(!state.spawnFailed && child.exitCode === null && child.signalCode === null,
            "The native host exited before completing its self-check");
          try {
            const bytes = await readFile(path.join(runtime, "native-result.json"), "utf8");
            requireCheck(bytes.length <= 16 * 1024, "The native observer report exceeded its bound");
            observed = JSON.parse(bytes);
          } catch (error) {
            if (error instanceof CheckFailure) throw error;
            if (error.code !== "ENOENT" && !(error instanceof SyntaxError)) throw new CheckFailure("The owned native observer report could not be read");
          }
          if (!observed) await delay(100);
        }
        requireCheck(observed, "The native host did not publish its guarded self-check result within 35 seconds");
        report.nativeEvidence = observed;
        requireCheck((await profileProcesses(profile, env)).length > 0, "WebView2 did not use the exact owned temporary profile");
        completed("owned_temporary_webview2_profile");
        requireCheck(!observed.host.failedStep, `The real native host failed its ${observed.host.failedStep || "unknown"} step`);
        const hostChecks = ["empty_registry", "prepared_two_views", "sandbox_attributes", "foreign_messages", "artifact_dispatch_ignored", "wrong_key_dispatch_ignored", "release_idempotent", "main_host_unchanged"];
        for (const name of hostChecks) requireCheck(observed.host.checks[name] === true, `The native host did not prove ${name}`);
        completed("native_host_real_prepare_and_release");
        requireCheck(observed.protocolOverflow === false && Array.isArray(observed.protocol) && observed.protocol.length === 3,
          "The real HTML protocol did not observe exactly two live views and one revoked view");
        for (const response of observed.protocol) requireCheck(response.strictCsp && response.noStore && response.nosniff,
          "An actual native HTML response omitted its production CSP, cache, or MIME guard");
        const live = observed.protocol.filter((response) => response.status === 200 && !response.emptyBody && !response.previouslyServed);
        const revoked = observed.protocol.filter((response) => response.status === 404 && response.emptyBody && response.previouslyServed);
        requireCheck(live.length === 2 && revoked.length === 1, "An actual previously served native HTML URL remained accessible after release");
        completed("actual_protocol_csp_and_released_url_denied", { liveResponses: 2, releasedStatus: 404 });
        stage = "artifact confinement probes";
        validateProbes(observed.host.results, 0, true, hits.length, observed.artifactInvokesDenied, observed.prepareInvokesSeen);
        requireCheck(observed.host.results.userActivationAtProbe === false, "The scripted native driver unexpectedly claimed user activation");
        requireCheck(hits.length === 0, "A native HTML probe reached the independent loopback canary");
        completed("actual_inline_counter", { value: 2, interaction: "scripted HTMLElement.click, no user activation" });
        completed("parent_storage_and_ipc_denied", {
          bridgePromise: observed.host.results.probes.bridgeInvoke,
          nativePrepareInvokes: observed.prepareInvokesSeen,
          artifactGuardRejections: observed.artifactInvokesDenied,
        });
        completed("network_and_local_file_denied", { canaryRequests: hits.length, localFileResponseObserver: "unavailable without CDP" });
        completed("popup_worker_form_and_top_navigation_denied", {
          activationSensitiveCoverage: "scripted only; browser fixtures required",
          formEvidence: observed.host.results.violations.includes("form-action") ? "form-action violation"
            : "actual submit attempted, independent canary requests zero, document remained on its protocol URL",
        });
        completed("rtc_immutable_and_blocked");
        completed("initial_about_blank_rtc_and_same_origin_sibling_denied", { nestedRealm: observed.host.results.probes.nestedRealm });
        completed("foreign_postmessages_ignored", { delivered: 3 });
        completed("forged_native_invoke_wrong_key_denied", { hostTransport: "sent", artifactTransport: observed.host.results.probes.rawNativeInvoke, nativeViews: 2 });
        completed("main_host_unchanged");
        return;
      }
      browser = await connectNative(chromium, profile, child, state);
      const deadline = Date.now() + stepTimeout;
      while (!page && Date.now() < deadline) {
        page = browser.contexts().flatMap((context) => context.pages()).find((candidate) => candidate.url() === hostUrl);
        if (!page) await delay(100);
      }
      requireCheck(page, "The actual native check main host was not found; rebuild the debug executable");
      const context = page.context();
      context.setDefaultTimeout(stepTimeout);
      requireCheck(context.pages().length === 1, "The isolated native check opened extra pages");
      requireCheck((await profileProcesses(profile, env)).length > 0, "WebView2 did not use the exact owned temporary profile");
      completed("owned_temporary_webview2_profile");
      let popups = 0;
      let formsDenied = 0;
      let localFileResponses = 0;
      let hostNavigations = 0;
      context.on("page", (popup) => { popups += 1; void popup.close().catch(() => {}); });
      page.on("console", (message) => {
        if (message.type() === "error") report.diagnostics.consoleErrors += 1;
        if (/blocked.*form|form.*blocked/i.test(message.text())) formsDenied += 1;
      });
      page.on("pageerror", () => { report.diagnostics.pageErrors += 1; });
      page.on("requestfailed", () => { report.diagnostics.failedRequests += 1; });
      page.on("response", (response) => {
        if (response.url() === fileUrl) localFileResponses += 1;
      });
      page.on("framenavigated", (frame) => {
        if (frame === page.mainFrame() && frame.url() !== hostUrl) hostNavigations += 1;
      });
      await page.waitForFunction(() => typeof window.__TAURI_INTERNALS__?.invoke === "function");
      const original = await page.evaluate(() => ({ host: document.getElementById("host").outerHTML, title: document.title }));
      requireCheck(await invoke(page, "html_check_count") === 0, "The private native HTML view registry was not empty");
      completed("native_host_and_empty_registry");
      session = await context.newCDPSession(page);
      await session.send("Network.enable");

      stage = "native prepare and actual protocol";
      const prepared = await invoke(page, "prepare_html_view", { document: {
        title: "Native HTML confinement check",
        html: documentSource({ canary: `http://127.0.0.1:${canaryPort}/canary/${runId}`, file: fileUrl }),
      } });
      requireCheck(typeof prepared?.token === "string" && /^[0-9a-f]{64}$/.test(prepared.token), "Native prepare did not return a bounded opaque token");
      tokens.add(prepared.token);
      requireCheck(prepared.url === `http://zest-html.localhost/${prepared.token}`, "Native prepare returned a non-HTML-protocol URL");
      requireCheck(await invoke(page, "html_check_count") === 1, "Native prepare did not add exactly one view");
      const sibling = await invoke(page, "prepare_html_view", { document: {
        title: "Same origin isolation canary", html: "<p id='sibling-canary'>Synthetic sibling canary</p>",
      } });
      requireCheck(typeof sibling?.token === "string" && /^[0-9a-f]{64}$/.test(sibling.token), "Native sibling prepare did not return an opaque token");
      tokens.add(sibling.token);
      requireCheck(sibling.url === `http://zest-html.localhost/${sibling.token}` && sibling.token !== prepared.token, "Native sibling prepare did not return a distinct same-origin protocol URL");
      requireCheck(await invoke(page, "html_check_count") === 2, "Native sibling prepare did not add exactly one view");
      const responsePromise = page.waitForResponse((response) => response.url() === prepared.url);
      void responsePromise.catch(() => {});
      const siblingResponse = page.waitForResponse((response) => response.url() === sibling.url);
      void siblingResponse.catch(() => {});
      await page.evaluate(({ url, siblingUrl }) => {
        const frame = document.createElement("iframe");
        frame.id = "artifact";
        frame.title = "Native HTML artifact";
        frame.setAttribute("sandbox", "allow-scripts");
        frame.style.cssText = "width:100%;height:600px;border:0";
        const counts = { ordinary: 0, forged: 0, string: 0 };
        window.__zestNativeMessageCounts = counts;
        window.addEventListener("message", (event) => {
          if (event.source !== frame.contentWindow) return;
          if (event.data?.kind === "zest-html-check-ordinary") counts.ordinary += 1;
          if (event.data?.kind === "zest-html-check-forged") counts.forged += 1;
          if (typeof event.data === "string") counts.string += 1;
        });
        frame.src = url;
        document.body.append(frame);
        const sibling = document.createElement("iframe");
        sibling.id = "same-origin-canary";
        sibling.setAttribute("sandbox", "allow-scripts");
        sibling.style.display = "none";
        sibling.src = siblingUrl;
        document.body.append(sibling);
      }, { url: prepared.url, siblingUrl: sibling.url });
      const response = await responsePromise;
      const siblingLoaded = await siblingResponse;
      requireCheck(siblingLoaded.status() === 200, "The same-origin native sibling did not load");
      checkCsp((await siblingLoaded.allHeaders())["content-security-policy"] || "");
      requireCheck(response.status() === 200, "The native HTML protocol did not serve the prepared view");
      const headers = await response.allHeaders();
      requireCheck(typeof headers["content-security-policy"] === "string", "The native HTML response omitted its authoritative CSP header");
      checkCsp(headers["content-security-policy"]);
      requireCheck(headers["cache-control"] === "no-store" && headers["x-content-type-options"] === "nosniff", "The native HTML protocol omitted its cache or MIME guards");
      const frame = page.frameLocator("#artifact");
      await frame.locator("#results").waitFor();
      const initial = JSON.parse(await frame.locator("#results").innerText());
      requireCheck(initial.phase === "ready" && initial.rtcBefore === true, "The actual HTML bootstrap did not block immutable RTC before user code");
      completed("native_html_response_csp_and_bootstrap");
      stage = "inline interaction";
      await frame.getByRole("button", { name: "Increment", exact: true }).click();
      await frame.getByRole("button", { name: "Increment", exact: true }).click();
      requireCheck(await frame.locator("#counter").innerText() === "2", "The actual inline counter did not respond to clicks");
      completed("actual_inline_counter", { value: 2 });

      stage = "artifact confinement probes";
      await frame.getByRole("button", { name: "Run confinement probes", exact: true }).click();
      let result;
      const probeDeadline = Date.now() + stepTimeout;
      while (Date.now() < probeDeadline) {
        result = JSON.parse(await frame.locator("#results").innerText());
        if (result.phase === "done") break;
        await delay(100);
      }
      requireCheck(result?.phase === "done", "The served HTML did not complete its confinement probes");
      await invoke(page, "html_check_report", { report: {
        checks: Object.fromEntries(["empty_registry", "prepared_two_views", "sandbox_attributes", "foreign_messages",
          "artifact_dispatch_ignored", "wrong_key_dispatch_ignored", "release_idempotent", "main_host_unchanged"].map((name) => [name, false])),
        results: result, failedStep: null,
      } });
      const guardEvidence = JSON.parse(await readFile(path.join(runtime, "native-result.json"), "utf8"));
      requireCheck(await invoke(page, "html_check_count") === 2, "An artifact bridge call changed the live registry");
      validateProbes(result, formsDenied, false, hits.length, guardEvidence.artifactInvokesDenied, guardEvidence.prepareInvokesSeen);
      requireCheck(popups === 0, "The HTML document opened a popup");
      requireCheck(!result.canaryExecuted && localFileResponses === 0 && hits.length === 0, "A network or local-file canary was accessed");
      requireCheck(page.frames().some((candidate) => candidate.url() === prepared.url), "A probe navigated the native HTML frame");
      completed("parent_storage_and_ipc_denied");
      completed("network_and_local_file_denied", { canaryRequests: hits.length, localFileResponses });
      completed("popup_worker_form_and_top_navigation_denied");
      completed("rtc_immutable_and_blocked");
      completed("initial_about_blank_rtc_and_same_origin_sibling_denied", { nestedRealm: result.probes.nestedRealm });

      stage = "foreign messages and forged native dispatch";
      await page.waitForFunction(() => {
        const counts = window.__zestNativeMessageCounts;
        return counts?.ordinary === 1 && counts.forged === 1 && counts.string === 1;
      });
      requireCheck(await invoke(page, "html_check_count") === 2, "A foreign message or artifact IPC call added a native HTML view");
      const rawSent = await page.evaluate(() => {
        const message = JSON.stringify({
          cmd: "prepare_html_view", callback: 2147483001, error: 2147483002,
          payload: { document: { title: "Forged host dispatch", html: "<p>forged</p>" } },
          options: { headers: {}, customProtocolIpcBlocked: true },
          __TAURI_INVOKE_KEY__: "zest-native-check-deliberately-wrong-key",
        });
        if (typeof window.chrome?.webview?.postMessage === "function") {
          window.chrome.webview.postMessage(message);
          return true;
        }
        if (typeof window.ipc?.postMessage === "function") { window.ipc.postMessage(message); return true; }
        return false;
      });
      requireCheck(rawSent, "The native host did not expose a real WebView2 transport for the wrong-key probe");
      const dispatchDeadline = Date.now() + 750;
      do {
        requireCheck(await invoke(page, "html_check_count") === 2, "A forged native invoke with a wrong key added an HTML view");
        await delay(100);
      } while (Date.now() < dispatchDeadline);
      requireCheck(hits.length === 0 && localFileResponses === 0, "A deferred canary request escaped confinement");
      completed("foreign_postmessages_ignored", { delivered: 3 });
      completed("forged_native_invoke_wrong_key_denied", { artifactTransport: result.probes.rawNativeInvoke, hostTransport: "sent", nativeViews: 2 });

      stage = "screenshot";
      const screenshot = path.join(artifacts, "native-html.png");
      const png = await page.screenshot({ path: screenshot, timeout: stepTimeout });
      requireCheck(png.subarray(0, 8).equals(Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]))
        && png.length >= 24 && png.readUInt32BE(16) > 0 && png.readUInt32BE(20) > 0, "The native screenshot was not an actual nonempty PNG");
      report.screenshot = screenshot;
      completed("actual_native_screenshot");

      stage = "native token revocation";
      await invoke(page, "release_html_view", { token: prepared.token });
      await invoke(page, "release_html_view", { token: prepared.token });
      tokens.delete(prepared.token);
      await invoke(page, "release_html_view", { token: sibling.token });
      tokens.delete(sibling.token);
      requireCheck(await invoke(page, "html_check_count") === 0, "Native release did not revoke the prepared token idempotently");
      const revokedResponse = page.waitForResponse((candidate) => candidate.url() === prepared.url);
      void revokedResponse.catch(() => {});
      await page.evaluate((url) => {
        document.getElementById("artifact").remove();
        const frame = document.createElement("iframe");
        frame.id = "revoked";
        frame.setAttribute("sandbox", "allow-scripts");
        frame.src = url;
        document.body.append(frame);
      }, prepared.url);
      const denied = await revokedResponse;
      requireCheck(denied.status() === 404, "A released native HTML URL remained accessible");
      requireCheck((await denied.body()).length === 0, "A released native HTML URL returned document bytes");
      const deniedHeaders = await denied.allHeaders();
      checkCsp(deniedHeaders["content-security-policy"] || "");
      completed("released_url_denied", { status: 404, nativeViews: 0 });

      stage = "main host integrity";
      const final = await page.evaluate(() => ({ host: document.getElementById("host").outerHTML, title: document.title }));
      requireCheck(page.url() === hostUrl && hostNavigations === 0 && final.host === original.host && final.title === original.title,
        "The native main host changed during the HTML probes");
      requireCheck(popups === 0 && context.pages().length === 1 && hits.length === 0 && localFileResponses === 0,
        "A native confinement check left an extra page or accessed a canary");
      completed("main_host_unchanged");
    };
    const onCancellation = new Promise((_, reject) => {
      if (cancellation.signal.aborted) reject(new CheckFailure("The native check was interrupted"));
      else cancellation.signal.addEventListener("abort", () => reject(new CheckFailure("The native check was interrupted")), { once: true });
    });
    await bounded(Promise.race([run(), onCancellation]), "The native HTML check exceeded 90 seconds", runTimeout);
    report.ok = true;
  } catch (error) {
    report.error = error instanceof CheckFailure ? error.message : `Native check failed during ${stage}; raw runtime diagnostics are suppressed`;
    report.failedStage = stage;
    if (runtime && ["native startup", "native host harness"].includes(stage)) {
      try {
        const allowed = new Set(["private_directory_validated", "context_created", "creating_webview", "webview_created", "runtime_created", "event_loop_ready", "main_host_served"]);
        report.diagnostics.nativeStartupStages = (await readFile(path.join(runtime, "native-startup.stages"), "utf8")).trim().split(/\r?\n/).filter((value) => allowed.has(value)).slice(0, 16);
      } catch {}
    }
    if (page && !page.isClosed() && artifacts && !report.screenshot) {
      try {
        const screenshot = path.join(artifacts, "failure.png");
        await page.screenshot({ path: screenshot, timeout: 2_000 });
        report.screenshot = screenshot;
      } catch {}
    }
  } finally {
    cancellation.abort();
    const cleanup = async (name, action) => {
      try { await action(); } catch { cleanupErrors.push(name); }
    };
    if (page && !page.isClosed()) {
      for (const token of tokens) await cleanup("release remaining native token", () => invoke(page, "release_html_view", { token }));
    }
    if (session) await cleanup("detach CDP session", () => bounded(session.detach(), "CDP session did not detach", 2_000));
    // Kill the native parent tree before disconnecting CDP, so descendants can
    // still be identified even if closing the connection exits WebView2 first.
    if (child) await cleanup("reap native and private WebView2 process trees", () => reapNative(child, childClosed, profile, env));
    if (browser) await cleanup("close CDP connection", () => bounded(browser.close(), "CDP connection did not close", 3_000));
    await cleanup("close canary server", () => closeServer(server));
    await cleanup("remove exact private runtime and canary directory", () => removeRuntime(runtime));
    process.off("SIGINT", interrupted);
    process.off("SIGTERM", interrupted);
    report.cleanup = { ok: cleanupErrors.length === 0, errors: cleanupErrors };
    if (cleanupErrors.length) report.ok = false;
  }
  if (artifacts) {
    report.report = path.join(artifacts, "report.json");
    try { await writeFile(report.report, `${JSON.stringify(report, null, 2)}\n`, { flag: "wx" }); }
    catch { report.ok = false; report.error = "Could not save the native check report"; }
  }
  return report;
}

try {
  const report = await main();
  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
  if (!report.ok) process.exitCode = 1;
} catch (error) {
  process.stdout.write(`${JSON.stringify({ ok: false, paidProviderCalls: 0, error: error instanceof CheckFailure ? error.message : "Native check setup failed; raw diagnostics are suppressed" })}\n`);
  process.exitCode = 1;
}
