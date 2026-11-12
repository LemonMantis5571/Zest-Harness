import { defineConfig } from "@playwright/test";

import { uiPort, uiUrl } from "./devServer.ts";

// ZEST_UI_PORT gives each checkout its own server; e2e/devServerSetup.ts
// refuses to run against a server that belongs to another checkout.
const port = uiPort();

export default defineConfig({
  testDir: "./e2e",
  fullyParallel: true,
  workers: 2,
  globalSetup: "./e2e/devServerSetup.ts",
  use: {
    baseURL: uiUrl(port),
    viewport: { width: 1280, height: 800 },
    trace: "retain-on-failure",
  },
  webServer: {
    command: `npm run dev -- --host 127.0.0.1 --port ${port} --strictPort`,
    url: uiUrl(port),
    reuseExistingServer: !process.env.CI,
  },
});
