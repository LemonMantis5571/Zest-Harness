import { expect, test, type Page } from "@playwright/test";

import { waitForFixtureBoot } from "./fixtureBoot";
import type { DesktopBackend } from "../src/lib/backend";

declare global {
  var __zestFixtureBackend: DesktopBackend | undefined;
  var __zestPauseAttentionLookup: (() => Promise<void>) | undefined;
  var __zestCompletedAttentionLookups: number | undefined;
  var __zestFixtureNotifications: string[];
  var __zestFixturePermissionCompleted: boolean;
}

const toasts = '[data-slot="toast"]';

/**
 * Zest toasts only while it is the focused window; otherwise the alert goes
 * to the OS. A headless page need not report focus, so pin it.
 */
async function forceFocus(page: Page) {
  await page.addInitScript(() => {
    Object.defineProperty(Document.prototype, "hasFocus", {
      configurable: true,
      value: () => true,
    });
  });
}

async function send(page: Page, text: string) {
  const composer = page.locator("#zest-composer-input");
  await expect(composer).toBeVisible();
  await composer.fill(text);
  await composer.press("Enter");
}

test("an approval in another chat toasts with the chat's name and an Open action", async ({
  page,
}) => {
  await forceFocus(page);
  await page.goto("/?fixture=1&scenario=background-approval");
  await send(page, "hello");

  const toast = page.locator(toasts).filter({ hasText: "Approval needed · Free chat" });
  await expect(toast).toBeVisible();
  await expect(toast).toContainText("Run the fixture tool in the background chat");
  await toast.getByRole("button", { name: "Open", exact: true }).click();

  await expect(toast).toHaveCount(0);
  await expect(page.getByRole("button", { name: /^Free chat\./ })).toHaveAttribute(
    "aria-current",
    "page"
  );
  await expect(page.getByRole("region", { name: "Approval needed" })).toBeVisible();
});

test("an approval toast waits for the user and closes when its chat is opened", async ({
  page,
}) => {
  await forceFocus(page);
  await page.goto("/?fixture=1&scenario=background-approval");
  await send(page, "hello");

  const toast = page.locator(toasts).filter({ hasText: "Approval needed · Free chat" });
  await expect(toast).toBeVisible();
  // Past Base UI's 5 s default: an approval holds the agent until answered.
  await page.waitForTimeout(6_000);
  await expect(toast).toBeVisible();

  await page.getByRole("button", { name: /^Free chat\./ }).click();
  await expect(page.getByRole("region", { name: "Approval needed" })).toBeVisible();
  await expect(toast).toHaveCount(0);
});

test("an approval in the chat on screen shows only its card", async ({ page }) => {
  await forceFocus(page);
  await page.goto("/?fixture=1&scenario=approval");
  await waitForFixtureBoot(page);
  await send(page, "exercise the boundary");

  await expect(page.getByRole("region", { name: "Approval needed" })).toBeVisible();
  // The decision awaits a focus check, so give a wrong toast time to appear.
  await page.waitForTimeout(500);
  await expect(page.locator(toasts)).toHaveCount(0);
});

test("automatic compaction leaves a divider in the transcript instead of a toast", async ({
  page,
}) => {
  // ui-stress reports a context window past the auto-compaction threshold, so
  // the turn's end compacts.
  await page.goto("/?fixture=1&scenario=ui-stress");
  const composer = page.locator("#zest-composer-input");
  await expect(composer).toBeVisible();
  await composer.fill("hello");
  await composer.press("Enter");

  const transcript = page.locator('[data-slot="message-scroller-viewport"]');
  await expect(transcript.getByText("Fixture echo: hello")).toBeVisible();
  const divider = transcript.getByRole("note");
  await expect(divider).toHaveText(
    "Conversation compacted automatically · restore point saved"
  );
  await expect(page.locator(toasts)).toHaveCount(0);
});

test("identically named chats keep separate approval alerts and Open actions", async ({ page }) => {
  await forceFocus(page);
  await page.goto("/?fixture=1&scenario=background-approval");
  await expect(page.locator("#zest-composer-input")).toBeVisible();
  await page.evaluate(async () => {
    const backend = globalThis.__zestFixtureBackend;
    if (!backend) throw new Error("Fixture backend is unavailable");
    const projects = await backend.listChatProjects();
    for (const threadId of ["fixture-free", "fixture-local"]) {
      const project = projects.find((item) => item.threads.some((thread) => thread.id === threadId));
      if (!project) throw new Error(`Missing fixture chat: ${threadId}`);
      await backend.renameThread(threadId, project.path, "Same title", project.path === null);
    }
  });
  await send(page, "start the first approval");
  const alerts = page.locator(toasts).filter({ hasText: "Approval needed · Same title" });
  await expect(alerts).toHaveCount(1);
  await page.evaluate(() => window.dispatchEvent(new CustomEvent(
    "zest:fixture-background-approval", { detail: "fixture-local" }
  )));
  await expect(alerts).toHaveCount(2);

  const opened: string[] = [];
  for (const remaining of [1, 0]) {
    await alerts.first().getByRole("button", { name: "Open", exact: true }).click();
    await expect(alerts).toHaveCount(remaining);
    const threadId = await page.evaluate(async () => {
      const backend = globalThis.__zestFixtureBackend;
      if (!backend) throw new Error("Fixture backend is unavailable");
      return (await backend.sessionInfo())?.threadId;
    });
    if (!threadId) throw new Error("No chat opened from the approval alert");
    opened.push(threadId);
  }
  expect(opened.sort()).toEqual(["fixture-free", "fixture-local"]);
  await expect(page.getByRole("region", { name: "Approval needed" })).toBeVisible();
});

test("a cancelled turn cannot create a late approval alert after chat lookup", async ({ page }) => {
  await forceFocus(page);
  await page.goto("/?fixture=1&scenario=background-approval");
  await send(page, "show a real approval first");
  const alert = page.locator(toasts).filter({ hasText: "Approval needed · Free chat" });
  await expect(alert).toBeVisible();
  await page.evaluate(async () => {
    const backend = globalThis.__zestFixtureBackend;
    if (!backend) throw new Error("Fixture backend is unavailable");
    await backend.cancelTurn("fixture-free");
  });
  await expect(alert).toHaveCount(0);

  const started = Promise.withResolvers<void>();
  const release = Promise.withResolvers<void>();
  await page.exposeFunction("__zestPauseAttentionLookup", async () => {
    started.resolve();
    await release.promise;
  });
  await page.evaluate(() => {
    const backend = globalThis.__zestFixtureBackend;
    const pause = globalThis.__zestPauseAttentionLookup;
    if (!backend || !pause) throw new Error("Fixture lookup barrier is unavailable");
    const list = backend.listChatProjects.bind(backend);
    globalThis.__zestCompletedAttentionLookups = 0;
    backend.listChatProjects = async () => {
      const projects = await list();
      await pause();
      globalThis.__zestCompletedAttentionLookups = (globalThis.__zestCompletedAttentionLookups ?? 0) + 1;
      return projects;
    };
    window.dispatchEvent(new CustomEvent("zest:fixture-background-approval", { detail: "fixture-free" }));
  });
  try {
    await started.promise;
    await page.evaluate(async () => {
      const backend = globalThis.__zestFixtureBackend;
      if (!backend) throw new Error("Fixture backend is unavailable");
      await backend.cancelTurn("fixture-free");
    });
  } finally {
    release.resolve();
  }
  await expect.poll(() => page.evaluate(() => globalThis.__zestCompletedAttentionLookups)).toBeGreaterThan(0);
  await page.evaluate(() => new Promise<void>((resolve) => {
    requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
  }));
  await expect(alert).toHaveCount(0);
});

for (const decision of ["Allow once", "Deny"]) {
for (const failOnce of [false, true]) {
  test(failOnce
    ? `a failed ${decision} restores the pending approval and its delayed alert`
    : `an accepted ${decision} cannot create a late approval alert before its result`, async ({ page }) => {
    await forceFocus(page);
    await page.goto(`/?fixture=1&scenario=background-approval&holdInteractionResults=1${failOnce ? "&failBackgroundApprovalOnce=1" : ""}`);
    await expect(page.locator("#zest-composer-input")).toBeVisible();
    const started = Promise.withResolvers<void>();
    const release = Promise.withResolvers<void>();
    await page.exposeFunction("__zestPauseAttentionLookup", async () => {
      started.resolve();
      await release.promise;
    });
    await page.evaluate(() => {
      const backend = globalThis.__zestFixtureBackend;
      const pause = globalThis.__zestPauseAttentionLookup;
      if (!backend || !pause) throw new Error("Fixture lookup barrier is unavailable");
      const list = backend.listChatProjects.bind(backend);
      let held = false;
      globalThis.__zestCompletedAttentionLookups = 0;
      backend.listChatProjects = async () => {
        const projects = await list();
        if (!held) {
          held = true;
          await pause();
          globalThis.__zestCompletedAttentionLookups = 1;
        }
        return projects;
      };
      window.dispatchEvent(new CustomEvent("zest:fixture-background-approval", { detail: "fixture-free" }));
    });
    try {
      await started.promise;
      const sidebar = page.locator("aside").first();
      await sidebar.getByRole("button", { name: /^Free chat\./ }).click();
      const approval = page.getByRole("region", { name: "Approval needed", exact: true });
      await approval.getByRole("button", { name: decision, exact: true }).click();
      if (failOnce) {
        await expect(page.locator(toasts).filter({ hasText: decision === "Deny" ? "Could not deny tool" : "Could not allow tool" })).toBeVisible();
        await expect(approval.getByRole("button", { name: decision, exact: true })).toBeEnabled();
      } else {
        await expect(page.getByRole("button", { name: "Stop", exact: true })).toBeVisible();
      }
      await sidebar.getByRole("button", { name: /^Local model chat\./ }).click();
    } finally {
      release.resolve();
    }
    await expect.poll(() => page.evaluate(() => globalThis.__zestCompletedAttentionLookups)).toBe(1);
    await page.evaluate(() => new Promise<void>((resolve) => {
      requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
    }));
    const alert = page.locator(toasts).filter({ hasText: "Approval needed · Free chat" });
    await expect(alert).toHaveCount(failOnce ? 1 : 0);
    if (failOnce) {
      await alert.getByRole("button", { name: "Open", exact: true }).click();
      const approval = page.getByRole("region", { name: "Approval needed", exact: true });
      await expect(approval).toBeVisible();
      await approval.getByRole("button", { name: decision, exact: true }).click();
      await expect(approval).toHaveCount(0);
    }
    if (decision === "Deny") {
      await page.evaluate(() => window.dispatchEvent(new CustomEvent(
        "zest:fixture-release-interaction-result", { detail: "fixture-free" }
      )));
      if (failOnce) {
        await expect(page.getByRole("button", { name: "Failed fixture tool fixture_tool", exact: true })).toBeVisible();
        await expect(page.getByRole("button", { name: "Stop", exact: true })).toHaveCount(0);
      }
    }
  });
}
}

for (const split of [false, true]) {
  test(`an answered question in ${split ? "split" : "single"} view cannot alert after delayed permission`, async ({ page }) => {
    const started = Promise.withResolvers<void>();
    const release = Promise.withResolvers<void>();
    await page.exposeFunction("__zestPauseAttentionLookup", async () => {
      started.resolve();
      await release.promise;
    });
    await page.addInitScript(() => {
      Object.defineProperty(Document.prototype, "hasFocus", { configurable: true, value: () => false });
      globalThis.__zestFixtureNotifications = [];
      globalThis.__zestFixturePermissionCompleted = false;
      Object.defineProperty(window, "Notification", { configurable: true, value: class {
        static permission = "default";
        static async requestPermission() {
          await globalThis.__zestPauseAttentionLookup?.();
          globalThis.__zestFixturePermissionCompleted = true;
          return "granted";
        }
        constructor(title: string) { globalThis.__zestFixtureNotifications.push(title); }
      } });
    });
    await page.goto("/?fixture=1&scenario=sidebar-status&holdInteractionResults=1");
    try {
      await started.promise;
      if (split) {
        await page.getByRole("button", { name: "Open split view", exact: true }).click();
        await page.getByRole("region", { name: "Right chat", exact: true })
          .getByRole("button", { name: /Pick a migration path/ }).click();
      } else {
        await page.locator("aside").first().getByRole("button", { name: /^Pick a migration path\./ }).click();
      }
      const question = page.locator('[data-slot="questionnaire"]');
      await expect(question).toContainText("Migrate in place or copy to a new table?");
      await question.getByText("In place", { exact: true }).click();
      await question.getByRole("button", { name: "Send answer", exact: true }).click();
      await expect(page.getByRole("button", { name: split ? "Stop response" : "Stop", exact: true })).toBeVisible();
    } finally {
      release.resolve();
    }
    await expect.poll(() => page.evaluate(() => globalThis.__zestFixturePermissionCompleted)).toBe(true);
    // The unanswered approval proves that permission completed and alerts can be delivered.
    await expect.poll(() => page.evaluate(() => globalThis.__zestFixtureNotifications))
      .toEqual(["Approval needed · Review the auth flow"]);
    await expect(page.locator('[data-slot="questionnaire"]')).toHaveCount(0);
    await page.evaluate(() => window.dispatchEvent(new CustomEvent(
      "zest:fixture-release-interaction-result", { detail: "fixture-question" }
    )));
    await expect(page.getByRole("button", { name: split ? "Stop response" : "Stop", exact: true })).toHaveCount(0);
  });
}

for (const outcome of ["retryable", "ended", "dropped"]) {
  test(`a ${outcome} question answer failure restores only a live retryable request`, async ({ page }) => {
    await forceFocus(page);
    await page.goto("/?fixture=1&scenario=sidebar-status&holdInteractionResults=1");
    await page.getByRole("button", { name: "Open split view", exact: true }).click();
    const right = page.getByRole("region", { name: "Right chat", exact: true });
    await right.getByRole("button", { name: /Pick a migration path/ }).click();
    await page.evaluate((failure) => {
      const backend = globalThis.__zestFixtureBackend;
      if (!backend) throw new Error("Fixture backend is unavailable");
      const resolve = backend.resolveQuestion.bind(backend);
      let failed = false;
      backend.resolveQuestion = async (...args) => {
        if (failed) return resolve(...args);
        failed = true;
        if (failure === "ended") await backend.cancelTurn("fixture-question");
        throw new Error(failure === "dropped" ? "fixture: no pending question" : "Fixture answer failed. Try again.");
      };
    }, outcome);
    const question = right.locator('[data-slot="questionnaire"]');
    await expect(question).toContainText("Migrate in place or copy to a new table?");
    await question.getByText("In place", { exact: true }).click();
    await question.getByRole("button", { name: "Send answer", exact: true }).click();
    if (outcome === "retryable") {
      await expect(page.locator(toasts).filter({ hasText: "Could not send answer" })).toBeVisible();
      await expect(question.getByRole("button", { name: "Send answer", exact: true })).toBeEnabled();
      await question.getByText("New table", { exact: true }).click();
      await question.getByRole("button", { name: "Send answer", exact: true }).click();
      await expect(question).toHaveCount(0);
      await page.evaluate(() => window.dispatchEvent(new CustomEvent(
        "zest:fixture-release-interaction-result", { detail: "fixture-question" }
      )));
      await expect(right.getByRole("button", { name: "Stop response", exact: true })).toHaveCount(0);
    } else {
      await expect(question).toHaveCount(0);
      if (outcome === "ended") await expect(right.getByRole("alert")).toHaveText("turn cancelled");
      else await expect(right.getByRole("button", { name: "Stop response", exact: true })).toBeVisible();
    }
  });
}

test("an approval behind a split chooser toasts and Open reveals the transcript", async ({ page }) => {
  await forceFocus(page);
  await page.goto("/?fixture=1&scenario=background-approval");
  await page.getByRole("button", { name: "Open split view", exact: true }).click();
  const right = page.getByRole("region", { name: "Right chat", exact: true });
  await right.getByRole("button", { name: /^Free chat/ }).click();
  await expect(right.getByRole("textbox", { name: "Message", exact: true })).toBeVisible();
  await right.getByRole("button", { name: /^Free chat/ }).click();
  await expect(right.getByRole("textbox", { name: "Find a chat", exact: true })).toBeVisible();
  await page.evaluate(() => window.dispatchEvent(new CustomEvent(
    "zest:fixture-background-approval", { detail: "fixture-free" }
  )));
  const alert = page.locator(toasts).filter({ hasText: "Approval needed · Free chat" });
  await expect(alert).toBeVisible();
  await alert.getByRole("button", { name: "Open", exact: true }).click();
  await expect(right.getByRole("textbox", { name: "Find a chat", exact: true })).toHaveCount(0);
  await expect(right.getByRole("button", { name: "Allow once", exact: true })).toBeVisible();
  await expect(alert).toHaveCount(0);
  await right.getByRole("button", { name: "Allow once", exact: true }).click();
  await expect(right.getByRole("button", { name: "Allow once", exact: true })).toHaveCount(0);
  await expect(right.getByRole("button", { name: "Stop response", exact: true })).toBeVisible();
});
