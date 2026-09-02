import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  handlePullRequestClick,
  pullRequestAnchorProps,
  shouldOpenPullRequestExternally,
} from "./pullRequestLink.ts";

function click(overrides: {
  metaKey?: boolean;
  ctrlKey?: boolean;
  shiftKey?: boolean;
  button?: number;
} = {}) {
  let prevented = false;
  let stopped = false;
  const event = {
    metaKey: false,
    ctrlKey: false,
    shiftKey: false,
    button: 0,
    ...overrides,
    preventDefault() {
      prevented = true;
    },
    stopPropagation() {
      stopped = true;
    },
  };
  return { event, prevented: () => prevented, stopped: () => stopped };
}

describe("shouldOpenPullRequestExternally", () => {
  it("keeps an unmodified left click in the app", () => {
    assert.equal(shouldOpenPullRequestExternally({ button: 0 }), false);
  });

  it("leaves for a modifier or middle click", () => {
    assert.equal(shouldOpenPullRequestExternally({ button: 0, metaKey: true }), true);
    assert.equal(shouldOpenPullRequestExternally({ button: 0, ctrlKey: true }), true);
    assert.equal(shouldOpenPullRequestExternally({ button: 0, shiftKey: true }), true);
    assert.equal(shouldOpenPullRequestExternally({ button: 1 }), true);
  });
});

describe("handlePullRequestClick", () => {
  it("opens the local view and swallows the row click", () => {
    const { event, prevented, stopped } = click();
    let opened = 0;
    assert.equal(
      handlePullRequestClick(event, () => {
        opened += 1;
      }),
      true
    );
    assert.equal(opened, 1);
    assert.equal(prevented(), true);
    assert.equal(stopped(), true);
  });

  it("leaves a modified click for the host URL and still stops the row", () => {
    const { event, prevented, stopped } = click({ metaKey: true });
    let opened = 0;
    assert.equal(
      handlePullRequestClick(event, () => {
        opened += 1;
      }),
      false
    );
    assert.equal(opened, 0);
    assert.equal(prevented(), false);
    assert.equal(stopped(), true);
  });

  it("leaves the host URL when there is no local opener", () => {
    const { event, prevented } = click();
    assert.equal(handlePullRequestClick(event), false);
    assert.equal(prevented(), false);
  });
});

describe("pullRequestAnchorProps", () => {
  it("keeps a real href and marks the link as in-app", () => {
    const props = pullRequestAnchorProps("https://github.com/zest/app/pull/13");
    assert.equal(props.href, "https://github.com/zest/app/pull/13");
    assert.equal(props.target, "_blank");
    assert.equal(props["data-internal-link"], "");
  });
});
