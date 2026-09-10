import { expect, test } from "@playwright/test";

test("btw streams followups then returns to an unchanged main conversation", async ({
  page,
}) => {
  await page.goto("/?fixture=1");
  await page
    .getByRole("button", { name: "Fifteen turns", exact: true })
    .click();
  const composer = page.locator("#zest-composer-input");
  await expect(composer).toBeVisible();
  const transcript = page.locator('[data-slot="message-scroller-viewport"]');
  const original = await transcript.innerText();
  await composer.fill("/btw Why this approach?");
  await composer.press("Enter");
  const panel = page.getByRole("dialog", { name: "Side conversation" });
  await expect(panel).toBeVisible();
  const input = panel.getByRole("textbox", { name: "Side question" });
  await expect(input).toBeEnabled();
  await expect(panel.getByRole("log")).toContainText(
    "The main conversation continues unchanged.",
  );
  await input.fill("What about alternatives?");
  await input.press("Enter");
  await expect(panel.getByRole("log")).toContainText(
    "Following up on “Why this approach?”",
  );
  await expect(input).toBeEnabled();
  await panel.getByRole("button", { name: "Close side conversation" }).click();
  await expect(panel).toBeHidden();
  await expect(composer).toHaveValue("");
  await expect(composer).toBeFocused();
  await expect(transcript).toHaveText(original, { useInnerText: true });

  await composer.fill("/btw");
  await composer.press("Enter");
  await expect(panel).toBeVisible();
  await expect(panel.getByRole("log")).not.toContainText("Why this approach?");
  await input.press("Escape");
  await expect(panel).toBeHidden();
  await expect(transcript).toHaveText(original, { useInnerText: true });
});

test("btw can open while the main task works without queueing its question", async ({
  page,
}) => {
  await page.goto("/?fixture=1");
  await expect(
    page.getByRole("button", { name: "Stop", exact: true }),
  ).toBeVisible();
  const composer = page.locator("#zest-composer-input");
  await composer.fill("/btw");
  await composer.press("Enter");
  const panel = page.getByRole("dialog", { name: "Side conversation" });
  await expect(panel).toContainText("Main task is still running");
  const input = panel.getByRole("textbox", { name: "Side question" });
  await expect(input).toBeEnabled();
  await input.fill("Independent side question");
  await input.press("Enter");
  await panel.getByRole("button", { name: "Close side conversation" }).click();
  await expect(panel).toBeHidden();
  await expect(
    page.getByRole("button", { name: "Stop", exact: true }),
  ).toBeVisible();
  await expect(
    page.locator('[data-slot="message-scroller-viewport"]'),
  ).not.toContainText("Independent side question");
});

test("the slash menu discovers btw and stopping keeps the question editable", async ({
  page,
}) => {
  await page.goto("/?fixture=1");
  const composer = page.locator("#zest-composer-input");
  await composer.fill("Keep this main draft /bt");
  const menu = page.getByRole("listbox", { name: "Commands", exact: true });
  await menu.getByRole("option", { name: /btw/ }).click();
  await expect(composer).toHaveValue("Keep this main draft ");
  const panel = page.getByRole("dialog", { name: "Side conversation" });
  const input = panel.getByRole("textbox", { name: "Side question" });
  await expect(input).toBeEnabled();
  await expect(input).toBeFocused();
  await input.fill("Stop this side answer");
  await input.press("Enter");
  await panel.getByRole("button", { name: "Stop side answer" }).click();
  await expect(input).toBeEnabled();
  await expect(input).toHaveValue("Stop this side answer");
  await expect(panel.getByRole("alert")).toContainText("Answer stopped");
  await input.press("Enter");
  await panel.getByRole("button", { name: "Close side conversation" }).click();
  await expect(panel).toBeHidden();
  await expect(composer).toBeEnabled();
  await expect(composer).toHaveValue("Keep this main draft ");
});
