import type { PreparedAttachment } from "./types";

/**
 * Ceilings on what one message may carry, mirroring `attachments.rs`.
 *
 * Rust enforces these per batch — it is the only place that can, since it is
 * what encodes the bytes. But a batch is one file-picker trip or one paste, and
 * nothing stops a user doing ten of those. The attachment list lives here, so
 * the limit across batches has to live here too.
 *
 * Duplicated constants rather than a round trip to fetch them: two numbers that
 * change roughly never are not worth a command, and a limit that needs an async
 * call cannot be checked before the expensive part has already happened.
 */
export const MAX_IMAGES = 8;
export const MAX_TOTAL_IMAGE_BYTES = 16 * 1024 * 1024;

/**
 * How many bytes a base64 string decodes to, without decoding it.
 *
 * Measuring the payload by decoding it would allocate the very copy the limit
 * exists to prevent.
 */
export function decodedLength(base64?: string | null): number {
  if (!base64) return 0;
  let padding = 0;
  for (let i = base64.length - 1; i >= 0 && base64[i] === "="; i -= 1) padding += 1;
  return Math.floor(base64.length / 4) * 3 - Math.min(padding, 2);
}

function isImage(attachment: PreparedAttachment): boolean {
  return attachment.kind === "image" && attachment.status !== "error";
}

export type Admission = {
  accepted: PreparedAttachment[];
  /** Name and reason for each refusal, for the caller to surface. */
  rejected: { name: string; reason: string }[];
};

/**
 * Decide which incoming attachments fit alongside the ones already attached.
 *
 * Non-images are always admitted: they are bounded elsewhere by a character cap,
 * and they are not what makes the payload large. Images are admitted in order
 * until a ceiling is reached, so the first ones a user chose are the ones kept.
 */
export function admitAttachments(
  existing: PreparedAttachment[],
  incoming: PreparedAttachment[]
): Admission {
  let images = existing.filter(isImage).length;
  let bytes = existing
    .filter(isImage)
    .reduce((sum, a) => sum + decodedLength(a.dataBase64), 0);

  const accepted: PreparedAttachment[] = [];
  const rejected: { name: string; reason: string }[] = [];

  for (const attachment of incoming) {
    if (!isImage(attachment)) {
      accepted.push(attachment);
      continue;
    }
    const size = decodedLength(attachment.dataBase64);
    if (images >= MAX_IMAGES) {
      rejected.push({
        name: attachment.name,
        reason: `Only ${MAX_IMAGES} images can be attached to one message.`,
      });
      continue;
    }
    if (bytes + size > MAX_TOTAL_IMAGE_BYTES) {
      rejected.push({
        name: attachment.name,
        reason: `Images are limited to ${MAX_TOTAL_IMAGE_BYTES / (1024 * 1024)} MB in total.`,
      });
      continue;
    }
    images += 1;
    bytes += size;
    accepted.push(attachment);
  }

  return { accepted, rejected };
}
