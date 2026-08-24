import type { DecisionGateView } from "./DecisionGateView";
import type { DispatchView } from "./DispatchView";
import type { ExternalSessionEvidenceView } from "./ExternalSessionEvidenceView";
import type { InboxMessageView } from "./InboxMessageView";
import type { LifecycleEntryView } from "./LifecycleEntryView";
import type { RetryStateView } from "./RetryStateView";
import type { WorktreeLineageView } from "./WorktreeLineageView";

export type OrchestrationView = { version: number, runId: string, taskId: string, parentThreadId: string, phase: string, dispatch: DispatchView | null, worktree: WorktreeLineageView, heartbeatAt: number | null, inbox: Array<InboxMessageView>, decisionGates: Array<DecisionGateView>, retry: RetryStateView, externalSession: ExternalSessionEvidenceView | null, externalSessionHistory: Array<ExternalSessionEvidenceView>, lifecycle: Array<LifecycleEntryView>, };
