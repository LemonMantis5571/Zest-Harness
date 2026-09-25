/**
 * V8 stores a string in one byte per character only while every character is
 * Latin-1. A string sliced out of one that is not stays two-byte, so a code
 * block parsed from a reply with a single em dash anywhere in its prose runs
 * every syntax-highlighting regex on the slower two-byte path: measured at
 * about 3x for a 220-line TypeScript block with shiki's JavaScript engine.
 *
 * Returns a fresh one-byte copy when the text allows it, and the text itself
 * when it holds a character above U+00FF, which no copy can make one-byte.
 * Copying costs well under a millisecond for a block that size.
 */
export function toOneByteIfLatin1(text: string): string {
  if (text.length === 0 || NON_LATIN1.test(text)) return text
  let copy = ""
  for (let start = 0; start < text.length; start += CHUNK) {
    const end = Math.min(start + CHUNK, text.length)
    const codes = new Array<number>(end - start)
    for (let i = start; i < end; i++) codes[i - start] = text.charCodeAt(i)
    // fromCharCode builds a one-byte string when every code is below 256.
    copy += String.fromCharCode.apply(null, codes)
  }
  return copy
}

const NON_LATIN1 = /[Ā-￿]/
/** Bounded so `apply` stays well inside every engine's argument limit. */
const CHUNK = 8192
