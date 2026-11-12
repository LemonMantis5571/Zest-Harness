import type { ProviderRow } from "./types";

/** A credentials file exists, or one showed up incomplete. */
export function loginSessionLooksPresent(row: {
  statusKind: string;
  detail: string;
}): boolean {
  return (
    row.statusKind === "ready" ||
    (row.statusKind === "not_logged_in" &&
      row.detail.toLowerCase().includes("incomplete"))
  );
}

/**
 * Reconnect starts while the stale file is still "ready". Treating that as
 * success made the waiting screen probe immediately, fail, and say the
 * provider was unavailable while `claude login` was still in the browser.
 */
export function loginSessionIsNew(
  row: Pick<ProviderRow, "statusKind" | "detail">,
  wasReadyAtStart: boolean
): boolean {
  if (!loginSessionLooksPresent(row)) return false;
  if (wasReadyAtStart && row.statusKind === "ready") return false;
  return true;
}
