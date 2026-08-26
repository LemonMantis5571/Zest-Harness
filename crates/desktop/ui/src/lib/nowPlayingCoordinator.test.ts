import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { createNowPlayingCoordinator } from "./nowPlayingCoordinator.ts";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

describe("now playing coordinator", () => {
  it("marks a poll stale when a newer action starts", async () => {
    const coordinator = createNowPlayingCoordinator<string>();
    const poll = deferred<string>();
    const action = deferred<string>();
    let reads = 0;

    const oldRead = coordinator.read(async () => {
      reads += 1;
      return poll.promise;
    });
    const next = coordinator.action(() => action.promise);

    poll.resolve("old song");
    const oldResult = await oldRead;
    assert.equal(oldResult.status, "success");
    assert.equal(oldResult.committed, false);

    action.resolve("new song");
    const nextResult = await next;
    assert.deepEqual(nextResult, {
      committed: true,
      status: "success",
      value: "new song",
    });
    assert.equal(reads, 1);
  });

  it("does not let a delayed refresh overwrite a second action", async () => {
    const coordinator = createNowPlayingCoordinator<string>();
    const first = deferred<string>();
    const refresh = deferred<string>();
    const second = deferred<string>();

    const firstAction = coordinator.action(() => first.promise);
    first.resolve("first song");
    assert.equal((await firstAction).committed, true);

    const delayedRefresh = coordinator.read(() => refresh.promise);
    const secondAction = coordinator.action(() => second.promise);

    refresh.resolve("stale refresh");
    const refreshResult = await delayedRefresh;
    assert.equal(refreshResult.status, "success");
    assert.equal(refreshResult.committed, false);

    second.resolve("second song");
    assert.deepEqual(await secondAction, {
      committed: true,
      status: "success",
      value: "second song",
    });
  });

  it("does not expose a stale error as the current state", async () => {
    const coordinator = createNowPlayingCoordinator<string>();
    const oldRead = deferred<string>();
    const action = deferred<string>();

    const oldResultPromise = coordinator.read(() => oldRead.promise);
    const actionResultPromise = coordinator.action(() => action.promise);

    oldRead.reject(new Error("old poll failed"));
    const oldResult = await oldResultPromise;
    assert.equal(oldResult.status, "error");
    assert.equal(oldResult.committed, false);

    action.resolve("current song");
    const actionResult = await actionResultPromise;
    assert.deepEqual(actionResult, {
      committed: true,
      status: "success",
      value: "current song",
    });
  });

  it("deduplicates polling while one read is pending", async () => {
    const coordinator = createNowPlayingCoordinator<string>();
    const pending = deferred<string>();
    let calls = 0;

    const first = coordinator.read(async () => {
      calls += 1;
      return pending.promise;
    });
    const second = coordinator.read(async () => {
      calls += 1;
      return "wrong operation";
    });

    assert.strictEqual(first, second);
    assert.equal(calls, 0);
    await Promise.resolve();
    assert.equal(calls, 1);

    pending.resolve("same song");
    assert.deepEqual(await first, {
      committed: true,
      status: "success",
      value: "same song",
    });
  });

  it("invalidates work on teardown", async () => {
    const coordinator = createNowPlayingCoordinator<string>();
    const pending = deferred<string>();
    const read = coordinator.read(() => pending.promise);

    await Promise.resolve();
    coordinator.dispose();
    pending.resolve("not committed");

    const result = await read;
    assert.equal(result.status, "success");
    assert.equal(result.committed, false);
    assert.equal((await coordinator.action(async () => "never")).status, "skipped");
  });
});
