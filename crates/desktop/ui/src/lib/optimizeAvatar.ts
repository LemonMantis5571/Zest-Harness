/** Max edge length for stored profile photos (header is ~28px). */
const AVATAR_MAX_EDGE = 128;
/** JPEG quality 0–1. */
const AVATAR_QUALITY = 0.82;
/** Reject optimized data URLs above this (chars); Rust also caps decoded bytes. */
const AVATAR_MAX_DATA_URL_CHARS = 80_000;
/** Allow large source files; we shrink before save. */
const AVATAR_MAX_SOURCE_BYTES = 8 * 1024 * 1024;

/**
 * Resize + JPEG-compress a picked image for the profile avatar.
 * Returns a `data:image/jpeg;base64,...` URL typically a few KB.
 */
export async function optimizeAvatarFile(file: File): Promise<string> {
  if (!file.type.startsWith("image/")) {
    throw new Error("Choose an image file");
  }
  if (file.size > AVATAR_MAX_SOURCE_BYTES) {
    throw new Error("Image too large (max 8MB before optimize)");
  }

  const bitmap = await createImageBitmap(file);
  try {
    const scale = Math.min(
      1,
      AVATAR_MAX_EDGE / Math.max(bitmap.width, bitmap.height, 1)
    );
    const w = Math.max(1, Math.round(bitmap.width * scale));
    const h = Math.max(1, Math.round(bitmap.height * scale));
    const canvas = document.createElement("canvas");
    canvas.width = w;
    canvas.height = h;
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("Could not optimize image");
    ctx.drawImage(bitmap, 0, 0, w, h);
    const dataUrl = canvas.toDataURL("image/jpeg", AVATAR_QUALITY);
    if (!dataUrl.startsWith("data:image/jpeg")) {
      throw new Error("JPEG encode failed");
    }
    if (dataUrl.length > AVATAR_MAX_DATA_URL_CHARS) {
      throw new Error("Optimized avatar still too large — try a simpler image");
    }
    return dataUrl;
  } finally {
    bitmap.close();
  }
}
