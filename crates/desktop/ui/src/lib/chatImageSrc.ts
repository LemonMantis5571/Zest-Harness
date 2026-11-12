import { convertFileSrc, isTauri } from "@tauri-apps/api/core";

import { safeImageSrc } from "./imageSrc.ts";
import { parseLocalImagePath } from "./localImagePath.ts";

/**
 * Anything ZoomableImage may load: a remote/data URL, or a local raster file
 * served through Tauri's asset protocol. file:// itself is rejected by CSP.
 */
export function resolveChatImageSrc(
  value: string | null | undefined
): string | null {
  const remote = safeImageSrc(value);
  if (remote) return remote;
  const path = parseLocalImagePath(value ?? "");
  if (!path || !isTauri()) return null;
  try {
    return convertFileSrc(path);
  } catch {
    return null;
  }
}
