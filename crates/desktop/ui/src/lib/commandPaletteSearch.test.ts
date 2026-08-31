import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  buildPaletteSections,
  flattenChats,
  formatPaletteAge,
  matchExcerpt,
  matchesQuery,
  mergeChatHits,
  shiftFilter,
} from "./commandPaletteSearch.ts";

describe("command palette search", () => {
  it("matches titles case-insensitively", () => {
    assert.equal(matchesQuery("Oceanic UI component", "oceanic"), true);
    assert.equal(matchesQuery("Oceanic UI component", "harness"), false);
    assert.equal(matchesQuery("Anything", ""), true);
  });

  it("formats compact ages", () => {
    const now = 1_700_000_000;
    assert.equal(formatPaletteAge(now - 12, now), "12s");
    assert.equal(formatPaletteAge(now - 11 * 3600, now), "11h");
    assert.equal(formatPaletteAge(now - 2 * 86400, now), "2d");
  });

  it("walks filters in a loop", () => {
    assert.equal(shiftFilter("all", 1), "chats");
    assert.equal(shiftFilter("settings", 1), "all");
    assert.equal(shiftFilter("all", -1), "settings");
  });

  it("groups recent chats and actions on All, settings only when asked", () => {
    const chats = flattenChats([
      {
        name: "zest",
        path: "/code/zest",
        active: true,
        threads: [
          {
            id: "t1",
            createdAt: 1,
            updatedAt: 20,
            title: "Image background plugin options",
            pinned: false,
            messageCount: 3,
          },
        ],
      },
      {
        name: "Free chats",
        path: null,
        active: false,
        threads: [
          {
            id: "t2",
            createdAt: 1,
            updatedAt: 10,
            title: "Untitled chat",
            pinned: false,
            messageCount: 1,
          },
        ],
      },
    ]);
    const actions = [
      { id: "go-back", label: "Go back", description: "Previous" },
      {
        id: "open-settings",
        label: "Open settings",
        description: "Configure Zest",
        group: "settings" as const,
      },
    ];

    const all = buildPaletteSections("all", "", chats, actions, []);
    assert.deepEqual(
      all.map((section) => section.id),
      ["chats", "actions"]
    );
    assert.equal(all[0]?.label, "Recent chats");
    assert.equal(all[0]?.items[0]?.kind === "chat" && all[0].items[0].item.title, "Image background plugin options");

    const settings = buildPaletteSections("settings", "", chats, actions, []);
    assert.deepEqual(
      settings.map((section) => section.id),
      ["settings"]
    );

    const filtered = buildPaletteSections("all", "settings", chats, actions, []);
    assert.deepEqual(
      filtered.map((section) => section.id),
      ["settings"]
    );
  });

  it("keeps a transcript snippet when merging title and body hits", () => {
    const snippet = matchExcerpt(
      "Please git pull the latest OceanicUI component branch and open a pull request.",
      "git pu"
    );
    assert.ok(snippet);
    assert.match(snippet, /git pull/i);

    const merged = mergeChatHits(
      [
        {
          id: "t1",
          title: "Local model chat",
          projectName: "zest",
          projectPath: "/code/zest",
          updatedAt: 10,
        },
      ],
      [
        {
          id: "t1",
          title: "Local model chat",
          projectName: "zest",
          projectPath: "/code/zest",
          updatedAt: 10,
          snippet,
          messageId: "u-local",
        },
      ]
    );
    assert.equal(merged.length, 1);
    assert.equal(merged[0]?.snippet, snippet);
    assert.equal(merged[0]?.messageId, "u-local");
  });
});
