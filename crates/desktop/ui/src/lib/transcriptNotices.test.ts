import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  addTranscriptNotice,
  compactionNoticeText,
  noticesByMessage,
} from "./transcriptNotices.ts";

describe("transcript notices", () => {
  it("says whether compaction trimmed tool output or summarized", () => {
    assert.equal(
      compactionNoticeText({ prunedOnly: true, resultsPruned: 1 }),
      "Trimmed 1 long tool result · full output kept in the restore point"
    );
    assert.equal(
      compactionNoticeText({ prunedOnly: true, resultsPruned: 3 }),
      "Trimmed 3 long tool results · full output kept in the restore point"
    );
    assert.equal(
      compactionNoticeText({ prunedOnly: false, resultsPruned: 0 }),
      "Conversation compacted automatically · restore point saved"
    );
  });

  it("keeps notices per thread without touching other threads", () => {
    const first = addTranscriptNotice({}, "a", { id: "n1", afterMessageId: "m1", text: "one" });
    const second = addTranscriptNotice(first, "b", { id: "n2", afterMessageId: "m9", text: "two" });
    const third = addTranscriptNotice(second, "a", { id: "n3", afterMessageId: "m2", text: "three" });

    assert.deepEqual(first, { a: [{ id: "n1", afterMessageId: "m1", text: "one" }] });
    assert.deepEqual(third.a?.map((notice) => notice.id), ["n1", "n3"]);
    assert.deepEqual(third.b?.map((notice) => notice.id), ["n2"]);
  });

  it("groups a thread's notices under the message they follow, in order", () => {
    const byMessage = noticesByMessage([
      { id: "n1", afterMessageId: "m1", text: "one" },
      { id: "n2", afterMessageId: "m2", text: "two" },
      { id: "n3", afterMessageId: "m1", text: "three" },
    ]);
    assert.deepEqual(byMessage.get("m1")?.map((notice) => notice.id), ["n1", "n3"]);
    assert.deepEqual(byMessage.get("m2")?.map((notice) => notice.id), ["n2"]);
    assert.equal(noticesByMessage(undefined).size, 0);
  });
});
