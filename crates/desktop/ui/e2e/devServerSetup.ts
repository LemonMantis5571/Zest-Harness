import { probeDevServer, reuseRefusal, uiPort, uiUrl } from "../devServer.ts";

// Runs after Playwright starts or reuses the dev server: a reused server must
// serve this checkout, or every spec would test another branch's code.
export default async function devServerSetup() {
  const port = uiPort();
  const probe = await probeDevServer(uiUrl(port));
  const refusal = probe.status === "free"
    ? { error: `nothing answers on port ${port}`, hint: "The dev server stopped during setup; run the tests again." }
    : reuseRefusal(probe, port);
  if (refusal) throw new Error(`${refusal.error}. ${refusal.hint}`);
}
