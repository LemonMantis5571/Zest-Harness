import { expect, test } from "@playwright/test";

test("Customize > Chat view owns the response display switch", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });

  await page.goto("/?fixture=1");
  await page.getByRole("button", { name: "Customize", exact: true }).click();
  await page.getByRole("button", { name: "Chat view", exact: true }).click();

  const toggle = page.getByRole("switch", { name: "Stream responses by complete blocks" });
  await expect(toggle).toHaveAttribute("aria-checked", "true");
  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-checked", "false");
  expect(
    await page.evaluate(() => localStorage.getItem("zest.responseBlockStreaming.v1"))
  ).toBe("false");

  await page.reload();
  await page.getByRole("button", { name: "Customize", exact: true }).click();
  await page.getByRole("button", { name: "Chat view", exact: true }).click();
  await expect(
    page.getByRole("switch", { name: "Stream responses by complete blocks" })
  ).toHaveAttribute("aria-checked", "false");

  await page.keyboard.press("Control+,");
  await expect(page.getByRole("dialog")).toBeVisible();
  await expect(page.getByRole("dialog").getByText("Response display")).toHaveCount(0);
  expect(errors).toEqual([]);
});
