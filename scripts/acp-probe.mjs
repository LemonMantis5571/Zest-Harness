// Capture the real wire shapes of Cursor's ACP mode before writing a provider.
//
// `cursor-agent acp` speaks JSON-RPC 2.0 over stdio, one message per line, with
// logs on stderr. Its documented flow is initialize -> authenticate ->
// session/new -> session/prompt -> session/update notifications ->
// session/request_permission. That flow maps closely onto the seam Zest already
// has for Codex (`ProviderInteractionHost` in crates/core/src/provider/mod.rs),
// so the open question is not *whether* to implement it but what the payloads
// actually look like: option ids on a permission request, the `sessionUpdate`
// kinds, and the shape of Cursor's own `cursor/*` extension methods.
//
// This probe answers that empirically. It drives one read-only turn, answers
// every server-initiated request (an unanswered blocking request such as
// `cursor/create_plan` hangs the agent), and writes every frame to a JSONL
// transcript. Where a response shape is not yet known it sends a best-effort
// body and records the agent's reaction, because a JSON-RPC error naming the
// bad field is itself the answer we are here to collect.
//
// Read-only by default: every permission request is refused unless
// ZEST_ACP_ALLOW=1, so a probe run cannot modify the workspace.
//
//   node ./scripts/acp-probe.mjs            # refuse everything, capture shapes
//   ZEST_ACP_ALLOW=1 node ./scripts/acp-probe.mjs
//
// Environment:
//   ZEST_ACP_COMMAND    cursor-agent executable or absolute path (default: cursor-agent)
//   ZEST_ACP_ARGS       extra args before `acp`, space-separated (e.g. "-e https://api2.cursor.sh")
//   ZEST_ACP_PROMPT     prompt for the single turn (default: a read-only question)
//   ZEST_ACP_MODE       session mode to request: ask | plan | agent (default: ask)
//   ZEST_ACP_CWD        session working directory (default: the repo root)
//   ZEST_ACP_CLIENT_FS  1 to advertise and serve fs/read_text_file + fs/write_text_file,
//                       which is what crates/core's own ACP client does
//   ZEST_ACP_ALLOW      1 to answer permission requests with allow-once
//   ZEST_ACP_LOGIN      1 to attempt `authenticate` (may open a browser)
//   ZEST_ACP_TIMEOUT_MS overall budget in ms (default: 120000)
//   ZEST_ACP_OUT        transcript path (default: outputs/acp-probe/<stamp>.jsonl)

import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const command = process.env.ZEST_ACP_COMMAND || "cursor-agent";
// Cursor documents auth and endpoint flags *before* the subcommand, as in
// `agent --api-key "$CURSOR_API_KEY" acp`, so extra args are a prefix.
const args = [...(process.env.ZEST_ACP_ARGS || "").split(" ").filter(Boolean), "acp"];
const mode = process.env.ZEST_ACP_MODE || "ask";
const cwd = path.resolve(process.env.ZEST_ACP_CWD || root);
// crates/core/src/tools/external_agent.rs advertises fs and terminal, so the
// agent asks Zest to do the reading and writing. Mirroring that here is how we
// find out whether Cursor takes the offer or keeps using its own tools.
const clientFs = process.env.ZEST_ACP_CLIENT_FS === "1";
const allow = process.env.ZEST_ACP_ALLOW === "1";
const attemptLogin = process.env.ZEST_ACP_LOGIN === "1";
const budgetMs = Number(process.env.ZEST_ACP_TIMEOUT_MS || 120_000);
const prompt =
  process.env.ZEST_ACP_PROMPT ||
  "Without editing anything, name the top-level directories here and describe this project in one paragraph.";

const stamp = new Date().toISOString().replace(/[:.]/g, "-");
const transcriptPath =
  process.env.ZEST_ACP_OUT || path.join(root, "outputs", "acp-probe", `${stamp}.jsonl`);
fs.mkdirSync(path.dirname(transcriptPath), { recursive: true });
const transcript = fs.createWriteStream(transcriptPath, { flags: "a" });

/** Every frame lands here, so a failed run is still a usable capture. */
function record(direction, payload) {
  transcript.write(JSON.stringify({ ts: Date.now(), direction, payload }) + "\n");
}

const seen = {
  serverNotifications: new Set(),
  serverRequests: new Set(),
  updateKinds: new Set(),
  permissionOptions: new Set(),
  errors: [],
};

// --- process ---------------------------------------------------------------

/**
 * Find the executable ourselves rather than letting a shell do it.
 *
 * Resolving up front is what makes "not installed" a fast, honest skip: a
 * `shell: true` fallback would spawn the shell successfully and only then print
 * "not recognized", turning a missing CLI into a run that stalls until every
 * request times out.
 */
function resolveCommand(name) {
  const extensions =
    process.platform === "win32"
      ? (process.env.PATHEXT || ".COM;.EXE;.BAT;.CMD").split(";").filter(Boolean)
      : [""];
  const candidates = (base) => [base, ...extensions.map((extension) => base + extension)];
  if (name.includes("/") || name.includes("\\") || path.isAbsolute(name)) {
    return candidates(path.resolve(root, name)).find((file) => fs.existsSync(file)) ?? null;
  }
  for (const dir of (process.env.PATH || "").split(path.delimiter).filter(Boolean)) {
    const found = candidates(path.join(dir, name)).find((file) => fs.existsSync(file));
    if (found) return found;
  }
  return null;
}

const executable = resolveCommand(command);

// A missing CLI is a skip, not a failure: this probe is opt-in tooling and must
// not break a checkout that has never installed cursor-agent.
if (!executable) {
  console.log(`acp probe: SKIPPED (${command} not found on PATH)`);
  console.log("install the Cursor CLI, then sign in with: cursor-agent login");
  transcript.end();
  process.exit(0);
}

/**
 * Spawn the resolved executable and wait to learn whether it started.
 *
 * `spawn` reports a bad executable asynchronously, so the failure arrives on
 * the error event rather than as a throw. A Windows `.cmd`/`.bat` shim cannot
 * be executed directly at all, so it goes through `cmd.exe /d /s /c` with the
 * arguments still passed as an array — `shell: true` would concatenate them
 * unescaped, which Node 24 deprecates for exactly that reason.
 */
function startAgent() {
  const isBatch = /\.(cmd|bat)$/i.test(executable);
  const program = isBatch ? process.env.COMSPEC || "cmd.exe" : executable;
  const programArgs = isBatch ? ["/d", "/s", "/c", executable, ...args] : args;
  const child = spawn(program, programArgs, {
    cwd: root,
    stdio: ["pipe", "pipe", "pipe"],
    windowsHide: true,
  });
  return new Promise((resolve) => {
    child.once("spawn", () => resolve({ child, error: null }));
    child.once("error", (error) => resolve({ child, error }));
  });
}

const { child, error: spawnFailed } = await startAgent();
if (spawnFailed) {
  console.log(`acp probe: SKIPPED (${executable} failed to start: ${spawnFailed.message})`);
  transcript.end();
  process.exit(0);
}

// Past startup, a pipe error must not take the probe down with an unhandled
// event: the transcript captured so far is the thing worth keeping.
child.on("error", (error) => record("spawn-error", error.message));
child.stdin.on("error", (error) => record("stdin-error", error.message));

// --- JSON-RPC over newline-delimited stdio ---------------------------------

let nextId = 1;
const pending = new Map();

function write(message) {
  record("client", message);
  child.stdin.write(JSON.stringify(message) + "\n");
}

function request(method, params) {
  const id = nextId++;
  const promise = new Promise((resolve) => {
    pending.set(id, resolve);
  });
  write({ jsonrpc: "2.0", id, method, params });
  return promise;
}

function respond(id, result) {
  write({ jsonrpc: "2.0", id, result });
}

function respondError(id, code, message) {
  write({ jsonrpc: "2.0", id, error: { code, message } });
}

/** Read an option id whatever the agent chose to call the field. */
function optionIdOf(option) {
  if (typeof option === "string") return option;
  return option?.optionId ?? option?.id ?? null;
}

/** Answer a permission request; the option list is what we are here to learn. */
function permissionResult(params) {
  const options = Array.isArray(params?.options) ? params.options : [];
  for (const option of options) {
    const id = optionIdOf(option);
    if (id) seen.permissionOptions.add(id);
  }
  const wanted = allow ? "allow" : "reject";
  const match =
    options.find((option) => optionIdOf(option)?.startsWith(`${wanted}-once`)) ??
    options.find((option) => optionIdOf(option)?.startsWith(wanted));
  const optionId = optionIdOf(match) ?? (allow ? "allow-once" : "reject-once");
  // Documented as an outcome envelope; if the agent rejects this body the error
  // it returns tells us the real field names, which is the point of the probe.
  return { outcome: { outcome: "selected", optionId } };
}

/**
 * Serve a file the agent asked *us* to touch, refusing anything outside cwd.
 *
 * The path check is the same rule `acp_relative_path` enforces in the Rust
 * client: an ACP agent naming a path is a request, not an authorization.
 */
function serveFile(id, params, write) {
  const target = path.resolve(cwd, params?.path ?? "");
  if (target !== cwd && !target.startsWith(cwd + path.sep)) {
    respondError(id, -32602, `path escapes the session cwd: ${params?.path}`);
    return;
  }
  try {
    if (!write) {
      respond(id, { content: fs.readFileSync(target, "utf8") });
      return;
    }
    if (!allow) {
      respondError(id, -32000, "probe is read-only; re-run with ZEST_ACP_ALLOW=1");
      return;
    }
    fs.mkdirSync(path.dirname(target), { recursive: true });
    fs.writeFileSync(target, params?.content ?? "", "utf8");
    respond(id, null);
  } catch (error) {
    respondError(id, -32000, error.message);
  }
}

/** Server-initiated requests. Anything left unanswered blocks the agent. */
function handleServerRequest(message) {
  const { id, method, params } = message;
  seen.serverRequests.add(method);
  switch (method) {
    case "session/request_permission":
      respond(id, permissionResult(params));
      return;
    case "fs/read_text_file":
      serveFile(id, params, false);
      return;
    case "fs/write_text_file":
      serveFile(id, params, true);
      return;
    case "cursor/ask_question": {
      const choices = params?.options ?? params?.choices ?? [];
      const answer = optionIdOf(choices[0]) ?? choices[0]?.label ?? null;
      respond(id, answer === null ? {} : { answer });
      return;
    }
    case "cursor/create_plan":
      // Blocking. Refuse rather than approve a plan nobody read.
      respond(id, { approved: false });
      return;
    default:
      // Say something. A silent unknown method is indistinguishable from a hang.
      respondError(id, -32601, `zest acp probe does not implement ${method}`);
  }
}

function handleNotification(message) {
  const { method, params } = message;
  seen.serverNotifications.add(method);
  if (method === "session/update") {
    const kind = params?.update?.sessionUpdate ?? params?.sessionUpdate ?? "(unnamed)";
    seen.updateKinds.add(String(kind));
  }
}

function handleMessage(message) {
  record("agent", message);
  if (message?.method && message.id !== undefined && message.id !== null) {
    handleServerRequest(message);
    return;
  }
  if (message?.method) {
    handleNotification(message);
    return;
  }
  const resolve = pending.get(message?.id);
  if (resolve) {
    pending.delete(message.id);
    resolve(message);
  }
}

let stdoutBuffer = "";
child.stdout.setEncoding("utf8");
child.stdout.on("data", (chunk) => {
  stdoutBuffer += chunk;
  let index = stdoutBuffer.indexOf("\n");
  while (index !== -1) {
    const line = stdoutBuffer.slice(0, index).replace(/\r$/, "").trim();
    stdoutBuffer = stdoutBuffer.slice(index + 1);
    if (line) {
      try {
        handleMessage(JSON.parse(line));
      } catch {
        record("agent-nonjson", line);
      }
    }
    index = stdoutBuffer.indexOf("\n");
  }
});

child.stderr.setEncoding("utf8");
child.stderr.on("data", (chunk) => record("stderr", chunk));

// --- one turn --------------------------------------------------------------

function failed(label, response) {
  if (!response) {
    seen.errors.push(`${label}: no response`);
    return true;
  }
  if (response.error) {
    seen.errors.push(`${label}: ${response.error.code} ${response.error.message}`);
    return true;
  }
  return false;
}

const timeout = (ms) => new Promise((resolve) => setTimeout(() => resolve(null), ms));
const within = (promise, ms) => Promise.race([promise, timeout(ms)]);

const initialize = await within(
  request("initialize", {
    protocolVersion: 1,
    clientCapabilities: {
      fs: { readTextFile: clientFs, writeTextFile: clientFs },
      terminal: false,
    },
    clientInfo: { name: "zest-acp-probe", version: "0" },
  }),
  30_000
);
failed("initialize", initialize);

const authMethods = initialize?.result?.authMethods ?? [];
if (attemptLogin && authMethods.length > 0) {
  failed("authenticate", await within(request("authenticate", { methodId: "cursor_login" }), 180_000));
}

const session = await within(request("session/new", { cwd, mcpServers: [] }), 60_000);
const sessionId = session?.result?.sessionId ?? session?.result?.session_id ?? null;
if (failed("session/new", session) || !sessionId) {
  if (!attemptLogin && authMethods.length > 0) {
    seen.errors.push(
      "session/new needs an authenticated CLI: run `cursor-agent login`, or re-run with ZEST_ACP_LOGIN=1"
    );
  }
} else {
  const setMode = await within(request("session/set_mode", { sessionId, modeId: mode }), 15_000);
  // Optional: an error here only tells us the mode must be set at session/new.
  if (setMode?.error) seen.errors.push(`session/set_mode: ${setMode.error.message}`);

  const turn = await within(
    request("session/prompt", { sessionId, prompt: [{ type: "text", text: prompt }] }),
    budgetMs
  );
  if (turn) {
    failed("session/prompt", turn);
  } else {
    seen.errors.push(`session/prompt: exceeded ${budgetMs}ms budget, cancelling`);
    write({ jsonrpc: "2.0", method: "session/cancel", params: { sessionId } });
    await timeout(5_000);
  }
}

/**
 * Stop the agent, including anything it spawned.
 *
 * On Windows the `.cmd` shim means our child is `cmd.exe`, and the real agent
 * is its grandchild: `child.kill()` reaps the wrapper and leaves the agent
 * holding the stdout pipe, which both orphans a process per run and keeps our
 * own event loop alive forever. `taskkill /t` is what actually ends the tree.
 */
async function stopAgent() {
  child.stdin.end();
  const exited = await Promise.race([
    new Promise((resolve) => child.once("exit", () => resolve(true))),
    timeout(5_000).then(() => false),
  ]);
  if (exited) return;
  if (process.platform === "win32") {
    const killer = spawn("taskkill", ["/pid", String(child.pid), "/t", "/f"], {
      stdio: "ignore",
      windowsHide: true,
    });
    await new Promise((resolve) => {
      killer.once("exit", resolve);
      killer.once("error", resolve);
    });
  } else {
    child.kill("SIGKILL");
  }
}

await stopAgent();
// Flush before exiting: the transcript is the whole point of a run, and an
// unflushed tail is exactly the part that describes how the run ended.
await new Promise((resolve) => transcript.end(resolve));

// --- what we learned -------------------------------------------------------

const list = (set) => (set.size ? [...set].sort().join(", ") : "(none observed)");
const permissions = allow ? "allow-once" : "refused";
console.log(`acp probe: transcript ${path.relative(root, transcriptPath)}`);
console.log(`  cwd                  ${cwd}`);
console.log(`  mode requested       ${mode} (permissions: ${permissions}, client fs: ${clientFs})`);
console.log(`  server requests      ${list(seen.serverRequests)}`);
console.log(`  server notifications ${list(seen.serverNotifications)}`);
console.log(`  session/update kinds ${list(seen.updateKinds)}`);
console.log(`  permission options   ${list(seen.permissionOptions)}`);
if (seen.errors.length > 0) {
  console.log("  errors");
  for (const error of seen.errors) console.log(`    - ${error}`);
}

// An agent that survived the kill would keep stdout open and hold the loop.
process.exit(0);
