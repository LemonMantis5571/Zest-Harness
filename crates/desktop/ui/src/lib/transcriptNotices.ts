/**
 * Quiet dividers in a chat's transcript for housekeeping Zest did between
 * turns, such as automatic compaction. They replace toasts: the user finds
 * them where it happened instead of being interrupted. They live only for
 * this app session.
 */
export type TranscriptNotice = {
  id: string;
  /** The divider is drawn after this message. */
  afterMessageId: string;
  text: string;
};

/** Notices per thread id. */
export type TranscriptNoticeMap = Readonly<Record<string, readonly TranscriptNotice[]>>;

export type CompactionOutcome = {
  prunedOnly: boolean;
  resultsPruned: number;
};

/** Trimming tool output and summarizing are different things to have happened, so they read differently. */
export function compactionNoticeText({ prunedOnly, resultsPruned }: CompactionOutcome) {
  if (prunedOnly) {
    const results = resultsPruned === 1 ? "result" : "results";
    return `Trimmed ${resultsPruned} long tool ${results} · full output kept in the restore point`;
  }
  return "Conversation compacted automatically · restore point saved";
}

export function addTranscriptNotice(
  notices: TranscriptNoticeMap,
  threadId: string,
  notice: TranscriptNotice
): TranscriptNoticeMap {
  return { ...notices, [threadId]: [...(notices[threadId] ?? []), notice] };
}

/** Group a thread's notices by the message they follow, in the order they happened. */
export function noticesByMessage(
  notices: readonly TranscriptNotice[] | undefined
): ReadonlyMap<string, readonly TranscriptNotice[]> {
  const byMessage = new Map<string, TranscriptNotice[]>();
  for (const notice of notices ?? []) {
    const existing = byMessage.get(notice.afterMessageId);
    if (existing) existing.push(notice);
    else byMessage.set(notice.afterMessageId, [notice]);
  }
  return byMessage;
}
