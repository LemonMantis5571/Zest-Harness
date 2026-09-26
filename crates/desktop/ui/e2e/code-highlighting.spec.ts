import { expect, test } from "@playwright/test";

// Each fence goes through the real chat path (Markdown -> CodeBlock -> Shiki),
// so a language dropped anywhere along the way renders uncolored and fails here.
const samples: Record<string, { code: string; label: string }> = {
  python: { code: "def f(x):\n    return x + 1", label: "python" },
  ruby: { code: "def greet(name)\n  puts \"hi #{name}\"\nend", label: "ruby" },
  rb: { code: "class Greeter; end", label: "ruby" },
  kotlin: { code: "fun main() { val x = 1 }", label: "kotlin" },
  swift: { code: "func main() { let x = 1 }", label: "swift" },
  php: { code: "<?php function f($x) { return $x; }", label: "php" },
  dockerfile: { code: "FROM node:24\nRUN npm ci", label: "docker" },
  powershell: { code: "$x = Get-ChildItem -Path .", label: "powershell" },
};

test("fenced code is highlighted for every language the highlighter ships", async ({ page }) => {
  await page.goto("/?fixture=1");
  const composer = page.locator("#zest-composer-input");
  // Let the seeded chat finish its canned boot turn, then work in a fresh chat
  // so every send runs at once instead of joining that chat's queue.
  const bootAnswer = page.getByText("streams tool calls and text over Tauri events.");
  await expect(bootAnswer).toBeVisible();
  await page.getByRole("button", { name: "New chat", exact: true }).first().click();
  await expect(bootAnswer).toHaveCount(0);

  for (const [tag, { code }] of Object.entries(samples)) {
    await composer.fill(`${tag} sample\n\n\`\`\`${tag}\n${code}\n\`\`\``);
    await composer.press("Enter");
    await expect(page.locator("[data-slot=code-block-code]").filter({ hasText: code.split("\n")[0] })).toHaveCount(1);
  }

  for (const [tag, { code, label }] of Object.entries(samples)) {
    const block = page.locator("[data-slot=code-block]").filter({
      has: page.locator("[data-slot=code-block-code]", { hasText: code.split("\n")[0] }),
    });
    await expect(block, `${tag} label`).toContainText(label, { ignoreCase: true });
    await expect(block.locator("[data-slot=code-block-token][style]").first(), `${tag} colored`).toBeVisible();
  }
});
