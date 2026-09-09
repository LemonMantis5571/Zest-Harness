import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { loginSessionIsNew, loginSessionLooksPresent } from "./loginWait.ts";

describe("loginSessionIsNew", () => {
  it("accepts a ready file that was not there at the start", () => {
    assert.equal(
      loginSessionIsNew({ statusKind: "ready", detail: "" }, false),
      true
    );
  });

  it("ignores a ready file that was already there when reconnect started", () => {
    assert.equal(
      loginSessionIsNew({ statusKind: "ready", detail: "" }, true),
      false
    );
  });

  it("still accepts an incomplete file as something new to probe", () => {
    assert.equal(
      loginSessionIsNew(
        { statusKind: "not_logged_in", detail: "session file is incomplete" },
        true
      ),
      true
    );
  });

  it("does not treat a missing session as present", () => {
    assert.equal(
      loginSessionLooksPresent({
        statusKind: "not_logged_in",
        detail: "claude login",
      }),
      false
    );
  });
});
