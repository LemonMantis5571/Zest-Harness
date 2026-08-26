import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { createFixtureBackend, type FixtureScenario } from "./fixtureBackend.ts";
import type { ChatEvent, DelegationEvent } from "./types.ts";

describe("fixture chat rename", () => {
  it("persists a trimmed title through the sidebar listing", async () => {
    const backend = createFixtureBackend();

    const [thread] = await backend.listThreads();
    const renamed = await backend.renameThread(thread.id, ".", "  Release checklist  ");

    assert.equal(renamed.title, "Release checklist");
    assert.equal(
      (await backend.listThreads()).find((item) => item.id === thread.id)?.title,
      "Release checklist"
    );
  });

  it("rejects an empty title", async () => {
    const backend = createFixtureBackend();
    const [thread] = await backend.listThreads();

    await assert.rejects(
      backend.renameThread(thread.id, ".", "   "),
      /chat title is empty/
    );
  });
});

describe("fixture free chats", () => {
  it("keeps free chats in Recent and out of the project tree", async () => {
    const backend = createFixtureBackend();

    const initial = await backend.listChatProjects();
    assert.equal(initial.filter((project) => project.path !== null).length, 1);
    assert.equal(initial.filter((project) => project.path === null).length, 1);

    const free = await backend.openProjectChat({ root: null, newThread: true });
    assert.equal(free.isFreeChat, true);

    const afterFree = await backend.listChatProjects();
    const project = afterFree.find((item) => item.path !== null);
    const recent = afterFree.find((item) => item.path === null);
    assert.equal(project?.threads.length, 0);
    assert.equal(recent?.threads.some((thread) => thread.id === free.threadId), true);
  });
});

describe("fixture delegation lifecycle", () => {
  it("keeps a newly created job awaiting explicit approval", async () => {
    const backend = createFixtureBackend();
    const job = await backend.createDelegationJob({
      parentThreadId: "thread-fixture",
      title: "Created job",
      objective: "Make the requested bounded change.",
      lane: "product",
      scope: ["src"],
      context: [],
      dependsOn: [],
      acceptanceChecks: [],
      worker: { kind: "provider", providerId: "fixture", model: null, effort: null },
      reviewer: { kind: "sameAsWorker" },
      chatId: "thread-fixture",
    });
    assert.equal(job.status, "awaiting_approval");
    assert.equal(job.approved, false);
  });

  it("drives a card through worker, reviewer, ready, and apply states", async () => {
    const backend = createFixtureBackend();
    const events: DelegationEvent[] = [];
    const unlisten = await backend.onDelegationEvent((event) => events.push(event));

    const [initial] = await backend.listDelegationJobs();
    assert.equal(initial.status, "awaiting_approval");
    assert.equal(initial.changedFileCount, 0);

    const retried = await backend.retryDelegationJob(initial.jobId);
    assert.equal(retried.status, "awaiting_approval");
    const ready = await backend.approveDelegationJob(initial.jobId);
    assert.equal(ready.status, "ready_to_apply");
    assert.deepEqual(
      events.map((event) => event.kind),
      [
        "approval_required",
        "worker_started",
        "worker_completed",
        "reviewer_started",
        "reviewer_completed",
        "ready_to_apply",
      ]
    );
    assert.deepEqual(ready.changedFiles, ["src/fixture.ts"]);
    assert.equal(ready.acceptanceChecks[0]?.status, "passed");

    const applied = await backend.applyDelegationJob(initial.jobId);
    assert.equal(applied.status, "accepted");
    assert.equal(events.at(-1)?.kind, "applied");
    assert.equal((await backend.getDelegationJob(initial.jobId)).status, "accepted");

    unlisten();
  });

  it("makes cancellation observable from the fixture board", async () => {
    const backend = createFixtureBackend();
    const events: DelegationEvent[] = [];
    await backend.onDelegationEvent((event) => events.push(event));
    const [job] = await backend.listDelegationJobs();

    const cancelled = await backend.cancelDelegationJob(job.jobId);
    assert.equal(cancelled.status, "cancelled");
    assert.equal(events.at(-1)?.kind, "cancelled");
  });

  it("groups target kinds and keeps unavailable targets actionable", async () => {
    const backend = createFixtureBackend();
    const targets = await backend.listDelegationTargets();
    assert.deepEqual(targets.map((target) => target.target.kind), ["provider", "externalAgent"]);
    assert.equal(targets[0]?.available, true);
    assert.equal(targets[1]?.available, false);
    assert.match(targets[1]?.error ?? "", /Reconnect|choose another target/);
  });

  it("returns only the bounded handoff summary for draft insertion", async () => {
    const backend = createFixtureBackend();
    const [job] = await backend.listDelegationJobs();
    const handoff = await backend.prepareDelegationHandoff(job.jobId);
    assert.equal(handoff.jobId, job.jobId);
    assert.equal(handoff.summary, "No worker summary is available yet.");
    assert.equal("messages" in handoff, false);
  });
});

describe("fixture plugins and workspace files", () => {
  it("keeps now playing opt-in and exposes a shallow file tree", async () => {
    const backend = createFixtureBackend();

    assert.equal((await backend.nowPlaying()).status, "disabled");
    await backend.setPluginEnabled("now-playing", true);
    const playing = await backend.nowPlaying();
    assert.equal(playing.title, "Midnight City");
    assert.equal(playing.artist, "M83");
    assert.equal(playing.artworkDataUrl?.startsWith("data:image/"), true);
    assert.equal(playing.volumePercent, 83);

    const paused = await backend.controlNowPlaying("toggle");
    assert.equal(paused.status, "paused");
    assert.equal((await backend.setNowPlayingVolume(42)).volumePercent, 42);

    const root = await backend.listWorkspaceFiles();
    assert.deepEqual(root.map((entry) => entry.name), ["src", "README.md", "Cargo.toml"]);

    const source = await backend.listWorkspaceFiles("src");
    assert.deepEqual(source.map((entry) => entry.name), ["main.ts", "lib.ts"]);
    const preview = await backend.readWorkspaceFile("src/main.ts");
    assert.equal(preview.content, "[fixture preview]");
    assert.equal(preview.byteCount, preview.content.length);
  });
});

describe("fixture safety scenarios", () => {
  async function scenarioEvents(scenario: FixtureScenario) {
    const backend = createFixtureBackend({ scenario });
    const events: ChatEvent[] = [];
    await backend.onChatEvent((event) => events.push(event));
    await backend.sendMessage("exercise the boundary");
    return { backend, events };
  }

  it("keeps an approval pending, then closes it exactly once when allowed", async () => {
    const { backend, events } = await scenarioEvents("approval");
    const approval = events.find((event) => event.kind === "approval_needed");
    assert.ok(approval && approval.kind === "approval_needed");
    assert.deepEqual(
      events.map((event) => event.kind),
      ["user", "assistant_start", "tool_call_start", "approval_needed"]
    );

    await backend.resolveApproval(approval.approval_id, "once");
    assert.deepEqual(events.map((event) => event.kind).slice(-3), [
      "tool_call_result",
      "text_delta",
      "done",
    ]);
    await assert.rejects(
      backend.resolveApproval(approval.approval_id, "once"),
      /no pending approval/
    );
    assert.equal(events.filter((event) => event.kind === "done").length, 1);
  });

  it("records a denial as an error tool result and still terminalizes the turn", async () => {
    const { backend, events } = await scenarioEvents("approval");
    const approval = events.find((event) => event.kind === "approval_needed");
    assert.ok(approval && approval.kind === "approval_needed");
    await backend.resolveApproval(approval.approval_id, "deny");
    const result = events.findLast((event) => event.kind === "tool_call_result");
    assert.ok(result && result.kind === "tool_call_result");
    assert.equal(result.isError, true);
    assert.equal(events.at(-1)?.kind, "done");
  });

  it("keeps a question pending until an answer is submitted", async () => {
    const { backend, events } = await scenarioEvents("question");
    const question = events.find((event) => event.kind === "question_needed");
    assert.ok(question && question.kind === "question_needed");
    assert.equal(events.at(-1)?.kind, "question_needed");
    await backend.resolveQuestion(question.question_id, "safe");
    assert.equal(events.at(-1)?.kind, "done");
    await assert.rejects(
      backend.resolveQuestion(question.question_id, "safe"),
      /no pending question/
    );
  });

  it("cancels an in-flight fixture turn once and never emits a completion", async () => {
    const { backend, events } = await scenarioEvents("cancel");
    await backend.cancelTurn();
    await backend.cancelTurn();
    assert.equal(events.at(-1)?.kind, "cancelled");
    assert.equal(events.filter((event) => event.kind === "cancelled").length, 1);
    assert.equal(events.some((event) => event.kind === "done"), false);
  });

  it("closes a failed tool event cleanly", async () => {
    const { events } = await scenarioEvents("tool-error");
    const result = events.find((event) => event.kind === "tool_call_result");
    assert.ok(result && result.kind === "tool_call_result");
    assert.equal(result.isError, true);
    assert.equal(events.at(-1)?.kind, "done");
  });
});

describe("fixture queued-message recovery", () => {
  it("claims only the oldest durable followup before delivering it", async () => {
    const backend = createFixtureBackend();
    const events: ChatEvent[] = [];
    await backend.onChatEvent((event) => events.push(event));
    await backend.sendMessage("first", [], "followup");
    await backend.sendMessage("second", [], "followup");
    await backend.resumeQueuedInputs("fixture");
    assert.deepEqual(events.map((event) => event.kind), [
      "input_queued",
      "input_queued",
      "input_removed",
      "user",
      "assistant_start",
      "text_delta",
      "text_delta",
      "done",
    ]);
    const info = await backend.sessionInfo();
    assert.equal(info?.pendingInputs.length, 1);
    assert.equal(info?.pendingInputs[0]?.text, "second");
  });
});
