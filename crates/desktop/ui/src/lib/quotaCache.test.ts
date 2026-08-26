import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  createProviderQuotaLoader,
  isProviderQuotaFresh,
  PROVIDER_QUOTA_TTL_MS,
  shouldFetchProviderQuota,
} from "./quotaCache.ts";
import type { ProviderQuotaSnapshot } from "./types.ts";

const snapshot: ProviderQuotaSnapshot = {
  checkedAt: 1_000,
  providers: [],
};

describe("provider quota cache policy", () => {
  it("does not call the provider before the panel asks for it", () => {
    let calls = 0;
    createProviderQuotaLoader(async () => {
      calls += 1;
      return snapshot;
    });

    assert.equal(calls, 0);
  });

  it("fetches once on the first panel open", async () => {
    let calls = 0;
    const loader = createProviderQuotaLoader(async () => {
      calls += 1;
      return snapshot;
    });

    const result = await loader.load(false, snapshot.checkedAt * 1000);

    assert.equal(calls, 1);
    assert.equal(result.kind, "fresh");
    assert.deepEqual(loader.getSnapshot(), snapshot);
  });

  it("reuses a snapshot inside the five-minute TTL", async () => {
    let calls = 0;
    const loader = createProviderQuotaLoader(async () => {
      calls += 1;
      return snapshot;
    });
    await loader.load(false, snapshot.checkedAt * 1000);

    assert.equal(
      isProviderQuotaFresh(snapshot, snapshot.checkedAt * 1000 + PROVIDER_QUOTA_TTL_MS - 1),
      true
    );
    const result = await loader.load(
      false,
      snapshot.checkedAt * 1000 + PROVIDER_QUOTA_TTL_MS - 1
    );

    assert.equal(shouldFetchProviderQuota(snapshot, snapshot.checkedAt * 1000 + 1), false);
    assert.equal(result.kind, "cached");
    assert.equal(calls, 1);
  });

  it("fetches an expired snapshot", async () => {
    let calls = 0;
    const loader = createProviderQuotaLoader(async () => {
      calls += 1;
      return snapshot;
    });
    await loader.load(false, snapshot.checkedAt * 1000);

    assert.equal(
      shouldFetchProviderQuota(snapshot, snapshot.checkedAt * 1000 + PROVIDER_QUOTA_TTL_MS),
      true
    );
    await loader.load(false, snapshot.checkedAt * 1000 + PROVIDER_QUOTA_TTL_MS);
    assert.equal(calls, 2);
  });

  it("manual refresh bypasses the TTL", async () => {
    let calls = 0;
    const loader = createProviderQuotaLoader(async () => {
      calls += 1;
      return snapshot;
    });
    await loader.load(false, snapshot.checkedAt * 1000);

    const result = await loader.load(true, snapshot.checkedAt * 1000 + 1_000);

    assert.equal(shouldFetchProviderQuota(snapshot, snapshot.checkedAt * 1000 + 1_000, true), true);
    assert.equal(result.kind, "fresh");
    assert.equal(calls, 2);
  });

  it("keeps the last good result when a refresh fails", async () => {
    let fail = false;
    const loader = createProviderQuotaLoader(async () => {
      if (fail) throw new Error("offline");
      return snapshot;
    });
    await loader.load(false, snapshot.checkedAt * 1000);
    fail = true;

    const result = await loader.load(true, snapshot.checkedAt * 1000 + 1_000);

    assert.equal(result.kind, "error");
    assert.deepEqual(result.snapshot, snapshot);
    assert.deepEqual(loader.getSnapshot(), snapshot);
  });

  it("rejects invalid timestamps instead of treating them as fresh", () => {
    assert.equal(isProviderQuotaFresh({ ...snapshot, checkedAt: 0 }, 1_000), false);
  });
});
