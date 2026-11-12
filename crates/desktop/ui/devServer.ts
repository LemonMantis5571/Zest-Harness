// Port and identity of the UI dev server, shared by vite.config.ts,
// playwright.config.ts and scripts/zest-control.mjs. Several checkouts of the
// repository can each run their own server on its own port (ZEST_UI_PORT), and
// a tool reuses a server only when it serves the tool's own checkout.

import path from "node:path";
import { fileURLToPath } from "node:url";

export const DEFAULT_UI_PORT = 1420;
export const IDENTITY_PATH = "/__zest_dev_server";
/** This checkout's UI root, the Vite root its dev server reports. */
export const UI_ROOT = path.dirname(fileURLToPath(import.meta.url));

export type DevServerIdentity = { app: "zest-ui"; root: string; pid: number };

export type DevServerProbe =
  | { status: "free" }
  | { status: "this-checkout"; root: string; pid: number }
  | { status: "other-checkout"; root: string; pid: number }
  | { status: "unidentified"; zest: boolean };

/** ZEST_UI_PORT, or 1420 when it is unset. */
export function uiPort(env: Record<string, string | undefined> = process.env): number {
  const raw = env.ZEST_UI_PORT?.trim();
  if (!raw) return DEFAULT_UI_PORT;
  const port = Number(raw);
  if (!/^\d+$/.test(raw) || port < 1 || port > 65_535) {
    throw new Error(`ZEST_UI_PORT must be a port number from 1 to 65535, not "${raw}".`);
  }
  return port;
}

export function uiUrl(port: number = uiPort()): string {
  return `http://127.0.0.1:${port}`;
}

export function sameRoot(left: string, right: string, platform: string = process.platform): boolean {
  const paths = platform === "win32" ? path.win32 : path.posix;
  const normalize = (value: string) => {
    const resolved = paths.resolve(value);
    return platform === "win32" ? resolved.toLowerCase() : resolved;
  };
  return normalize(left) === normalize(right);
}

function parseIdentity(text: string): DevServerIdentity | null {
  try {
    const value: unknown = JSON.parse(text);
    if (typeof value !== "object" || value === null) return null;
    const { app, root, pid } = value as Record<string, unknown>;
    return app === "zest-ui" && typeof root === "string" && typeof pid === "number" ? { app, root, pid } : null;
  } catch {
    return null;
  }
}

/** What answers at `url`, and whether it is this checkout's dev server. */
export async function probeDevServer(url: string, root: string = UI_ROOT): Promise<DevServerProbe> {
  let identity: DevServerIdentity | null;
  try {
    const response = await fetch(`${url}${IDENTITY_PATH}`, { signal: AbortSignal.timeout(2_000) });
    identity = parseIdentity(await response.text());
  } catch {
    return { status: "free" };
  }
  if (identity) {
    const status = sameRoot(identity.root, root) ? "this-checkout" : "other-checkout";
    return { status, root: identity.root, pid: identity.pid };
  }
  // A Zest UI from before the identity endpoint still serves the app shell.
  try {
    const body = await (await fetch(url, { signal: AbortSignal.timeout(2_000) })).text();
    return { status: "unidentified", zest: body.includes('id="root"') };
  } catch {
    return { status: "unidentified", zest: false };
  }
}

/** Why a server that is not this checkout's cannot be reused, with the way out. */
export function reuseRefusal(probe: DevServerProbe, port: number): { error: string; hint: string } | null {
  const elsewhere = `Stop it, or pick a free port for this checkout, e.g. ZEST_UI_PORT=${port === 65_535 ? port - 1 : port + 1}.`;
  switch (probe.status) {
    case "free":
    case "this-checkout":
      return null;
    case "other-checkout":
      return {
        error: `port ${port} serves the Zest UI of another checkout: ${probe.root} (pid ${probe.pid})`,
        hint: elsewhere,
      };
    case "unidentified":
      return {
        error: probe.zest
          ? `port ${port} serves a Zest UI that does not say which checkout it belongs to`
          : `port ${port} is in use by another program`,
        hint: elsewhere,
      };
  }
}
