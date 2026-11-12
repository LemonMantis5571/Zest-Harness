import assert from "node:assert/strict";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { probeDevServer, reuseRefusal, sameRoot, UI_ROOT } from "../crates/desktop/ui/devServer.ts";
import { COMMANDS, devServer, fixtureScenarios, parseArgs, statePath, validateScenario } from "./zest-control.mjs";

test("parses positional args, value flags, boolean flags, and --", () => {
  assert.deepEqual(parseArgs(["send", "hello", "world", "--no-wait"]), {
    command: "send",
    args: ["hello", "world"],
    options: { noWait: true },
  });
  assert.deepEqual(parseArgs(["click", "New chat", "--role", "tab", "--nth=2", "--exact"]), {
    command: "click",
    args: ["New chat"],
    options: { role: "tab", nth: "2", exact: true },
  });
  assert.deepEqual(parseArgs(["eval", "--", "--not-a-flag"]).args, ["--not-a-flag"]);
  assert.equal(parseArgs([]).command, "help");
});

test("a value flag at the end of argv becomes a boolean instead of eating nothing", () => {
  assert.deepEqual(parseArgs(["start", "--scenario"]).options, { scenario: true });
});

test("UI measurements retain the selector and viewport dimensions", () => {
  assert.deepEqual(parseArgs(["inspect", "ui-quality", "--selector", '[role="dialog"]']), {
    command: "inspect", args: ["ui-quality"], options: { selector: '[role="dialog"]' },
  });
  assert.deepEqual(parseArgs(["viewport", "840", "560"]), {
    command: "viewport", args: ["840", "560"], options: {},
  });
});

test("HTML interaction commands keep labels and frame selection explicit", () => {
  assert.deepEqual(parseArgs(["html-click", "Increment", "--nth", "1"]), {
    command: "html-click", args: ["Increment"], options: { nth: "1" },
  });
  assert.deepEqual(parseArgs(["html-value", "Amount", "42"]), {
    command: "html-value", args: ["Amount", "42"], options: {},
  });
  assert.ok("html-click" in COMMANDS);
  assert.ok("html-value" in COMMANDS);
});

test("the session file is per checkout and outside the repository", () => {
  const file = statePath("D:/Code/Zest-Harness");
  assert.equal(file, statePath("d:/code/zest-harness"));
  assert.notEqual(file, statePath("D:/Code/Other"));
  assert.ok(file.startsWith(path.join(os.tmpdir(), "zest-control")));
});

test("scenarios come from the fixture backend source", () => {
  const scenarios = fixtureScenarios();
  assert.ok(scenarios, "FixtureScenario union not found in fixtureBackend.ts");
  for (const name of ["approval", "split-streaming", "provider-picker"]) {
    assert.ok(scenarios.includes(name), `${name} missing from ${scenarios.join(", ")}`);
  }
  assert.deepEqual(fixtureScenarios('type FixtureScenario = "a" | "b-c";'), ["a", "b-c"]);
  assert.equal(fixtureScenarios("no union here"), null);
});

test("unknown scenarios are rejected with the known list", () => {
  const known = ["approval", "cancel"];
  assert.equal(validateScenario("approval", known), null);
  assert.equal(validateScenario("none", known), null);
  assert.equal(validateScenario(undefined, known), null);
  assert.match(validateScenario("aproval", known), /unknown scenario "aproval". Known: approval, cancel/);
  // Without a readable source, any name is passed through to the page.
  assert.equal(validateScenario("anything", null), null);
});

test("every command has help text", () => {
  for (const [name, text] of Object.entries(COMMANDS)) {
    assert.ok(text.length > 10, `${name} needs a description`);
  }
  for (const name of ["doctor", "check", "start", "stop", "send", "inspect", "errors", "snapshot", "screenshot"]) {
    assert.ok(name in COMMANDS, `${name} missing`);
  }
});

test("ZEST_UI_PORT picks the dev server port, 1420 by default", () => {
  assert.deepEqual(devServer({}), { port: 1420, url: "http://127.0.0.1:1420" });
  assert.deepEqual(devServer({ ZEST_UI_PORT: " 1431 " }), { port: 1431, url: "http://127.0.0.1:1431" });
  for (const bad of ["abc", "0", "65536", "14.2", "-1", "1420x"]) {
    assert.throws(() => devServer({ ZEST_UI_PORT: bad }), /ZEST_UI_PORT must be a port number/, bad);
  }
});

test("checkout roots compare by resolved path, case-insensitively only on Windows", () => {
  assert.ok(sameRoot("D:/Code/Zest/crates/desktop/ui", String.raw`d:\code\zest\crates\desktop\ui`, "win32"));
  assert.ok(!sameRoot("/code/Zest/crates/desktop/ui", "/code/zest/crates/desktop/ui", "linux"));
  assert.ok(!sameRoot(UI_ROOT, path.join(UI_ROOT, "..", "..", "..", ".claude", "worktrees", "other", "crates", "desktop", "ui")));
});

/** Serve `handler` on a free loopback port for the length of `run`. */
async function withServer(handler, run) {
  const server = http.createServer(handler);
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  try {
    return await run(server.address().port);
  } finally {
    await new Promise((resolve) => server.close(resolve));
  }
}

const identityServer = (root) => (request, response) => {
  if (request.url === "/__zest_dev_server") {
    response.setHeader("content-type", "application/json");
    response.end(JSON.stringify({ app: "zest-ui", root, pid: 4242 }));
  } else {
    response.end('<div id="root"></div>');
  }
};

test("a dev server is reused only when it serves this checkout", async () => {
  await withServer(identityServer(UI_ROOT.replaceAll("\\", "/")), async (port) => {
    const probe = await probeDevServer(`http://127.0.0.1:${port}`);
    assert.deepEqual(probe, { status: "this-checkout", root: UI_ROOT.replaceAll("\\", "/"), pid: 4242 });
    assert.equal(reuseRefusal(probe, port), null);
  });

  const other = "D:/Code/Zest-Harness/.claude/worktrees/other/crates/desktop/ui";
  await withServer(identityServer(other), async (port) => {
    const probe = await probeDevServer(`http://127.0.0.1:${port}`);
    assert.equal(probe.status, "other-checkout");
    const refusal = reuseRefusal(probe, port);
    assert.equal(refusal.error, `port ${port} serves the Zest UI of another checkout: ${other} (pid 4242)`);
    assert.match(refusal.hint, new RegExp(`ZEST_UI_PORT=${port + 1}`));
  });
});

test("a server that cannot say which checkout it serves is refused", async () => {
  // A Zest UI from before the identity endpoint falls back to the app shell.
  await withServer((_request, response) => response.end('<div id="root"></div>'), async (port) => {
    const probe = await probeDevServer(`http://127.0.0.1:${port}`);
    assert.deepEqual(probe, { status: "unidentified", zest: true });
    assert.match(reuseRefusal(probe, port).error, /does not say which checkout/);
  });
  await withServer((_request, response) => response.end("something else"), async (port) => {
    const probe = await probeDevServer(`http://127.0.0.1:${port}`);
    assert.deepEqual(probe, { status: "unidentified", zest: false });
    assert.match(reuseRefusal(probe, port).error, new RegExp(`port ${port} is in use by another program`));
  });
});

test("a free port is reported free, so the tools start their own server", async () => {
  const port = await withServer(() => {}, async (free) => free);
  const probe = await probeDevServer(`http://127.0.0.1:${port}`);
  assert.deepEqual(probe, { status: "free" });
  assert.equal(reuseRefusal(probe, port), null);
});
