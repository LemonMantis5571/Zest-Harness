import { safeHttpUrl } from "./externalLinks.ts";

const DATA_IMAGE_RE = /^data:image\/(png|jpe?g|gif|webp|avif|bmp)(;|,)/i;

/** Images we will render in chat: http(s) or a raster data URL. */
export function safeImageSrc(value: string | null | undefined): string | null {
  const src = value?.trim();
  if (!src) return null;
  if (src.startsWith("data:")) {
    return DATA_IMAGE_RE.test(src) ? src : null;
  }
  return safeHttpUrl(src);
}
