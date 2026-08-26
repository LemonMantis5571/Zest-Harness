/** Messages between the main thread and the Shiki worker. */

export type HighlightRequest = {
  /** Correlates the answer. A late reply for a superseded request is dropped. */
  id: number;
  code: string;
  lang: string;
};

export type HighlightResponse =
  | { id: number; ok: true; html: string }
  | { id: number; ok: false; error: string };
