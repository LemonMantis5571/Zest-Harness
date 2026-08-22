import assert from "node:assert/strict";
import test from "node:test";

import {
  countThinkingSteps,
  lastThinkingLine,
  thinkingSentences,
  thinkingTraceRows,
  thinkingSummaryLabel,
  thoughtForLabel,
} from "./thinkingSummary.ts";

/**
 * The shipped symptom: twenty summarized steps rendered as a column of headings
 * taller than the viewport, pushing the actual answer off screen.
 */
const STREAM = [
  "**Planning delegation with context gathering**",
  "",
  "Looking at what the worker needs to know.",
  "",
  "**Drafting self-contained delegation task**",
  "",
  "Writing it so it stands alone.",
  "",
  "**Analyzing migration and test update requirements**",
  "",
  "Checking which tests move.",
].join("\n");

test("the newest step title is what the single line shows", () => {
  assert.equal(
    lastThinkingLine(STREAM),
    "Analyzing migration and test update requirements"
  );
});

test("a half-streamed title still resolves to the previous complete one", () => {
  assert.equal(lastThinkingLine(`${STREAM}\n\n**Refining hir`), "Analyzing migration and test update requirements");
});

test("untitled prose falls back to its own tail rather than going blank", () => {
  const prose = "Let me look at the config.\nThe provider list comes from Rust.";
  assert.equal(lastThinkingLine(prose), "The provider list comes from Rust.");
});

test("stray emphasis never reaches the visible line as literal asterisks", () => {
  assert.equal(lastThinkingLine("Checking the **gateway** now"), "Checking the gateway now");
});

test("empty thinking yields an empty line, not a crash", () => {
  assert.equal(lastThinkingLine(""), "");
  assert.equal(lastThinkingLine("\n\n  \n"), "");
});

test("step counting drives the settled summary label", () => {
  assert.equal(countThinkingSteps(STREAM), 3);
  assert.equal(thinkingSummaryLabel(STREAM), "Thought through 3 steps");
  assert.equal(thinkingSummaryLabel("**Only one**"), "Thought through 1 step");
  assert.equal(thinkingSummaryLabel("plain prose"), "Thought about this");
});

test("thinking rows pair each title with the prose that explains it", () => {
  assert.deepEqual(thinkingTraceRows(STREAM), [
    {
      primary: "Planning delegation with context gathering",
      secondary: "Looking at what the worker needs to know.",
      kind: "step",
    },
    {
      primary: "Drafting self-contained delegation task",
      secondary: "Writing it so it stands alone.",
      kind: "step",
    },
    {
      primary: "Analyzing migration and test update requirements",
      secondary: "Checking which tests move.",
      kind: "step",
    },
  ]);
});

test("untitled thinking still produces a useful detail row", () => {
  assert.deepEqual(thinkingTraceRows("Checking the **gateway** now."), [
    { primary: "Checking the gateway now.", kind: "detail" },
  ]);
});

test("sentences join a step with its detail on one clamped line", () => {
  assert.deepEqual(thinkingSentences(STREAM), [
    "Planning delegation with context gathering — Looking at what the worker needs to know.",
    "Drafting self-contained delegation task — Writing it so it stands alone.",
    "Analyzing migration and test update requirements — Checking which tests move.",
  ]);
});

test("a step with no detail becomes its title alone", () => {
  assert.deepEqual(thinkingSentences("**Only one**"), ["Only one"]);
});

test("thinking with nothing in it produces no sentences", () => {
  assert.deepEqual(thinkingSentences("   \n\n "), []);
});

test("a measured duration is reported in seconds and minutes", () => {
  assert.equal(thoughtForLabel(0, STREAM), "Thought for 1s");
  assert.equal(thoughtForLabel(12, STREAM), "Thought for 12s");
  assert.equal(thoughtForLabel(59, STREAM), "Thought for 59s");
  assert.equal(thoughtForLabel(60, STREAM), "Thought for 1m");
  assert.equal(thoughtForLabel(95, STREAM), "Thought for 1m 35s");
});

/** A reopened chat has the reasoning but not the clock; do not invent one. */
test("an unmeasured turn describes its steps instead of a duration", () => {
  assert.equal(thoughtForLabel(null, STREAM), "Thought through 3 steps");
  assert.equal(thoughtForLabel(null, "plain prose"), "Thought about this");
});
