import assert from "node:assert/strict";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { COMMANDS, fixtureScenarios, parseArgs, statePath, validateScenario } from "./zest-control.mjs";

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
