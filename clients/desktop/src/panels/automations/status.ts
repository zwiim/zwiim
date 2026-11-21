import type { RunItemView, RunStatus } from "@zwiim/sdk";

/** The status dot an item or a playbook shows (design §4). */
export type Dot = "idle" | "work" | "need" | "done" | "off" | "err";

export interface RunCounts {
  total: number;
  sent: number;
  need: number;
}

/** What an item's tile says about it, and the overlay on its page when it waits on the person. */
export function itemStatus(item: RunItemView): { dot: Dot; label: string; overlay: string | null } {
  switch (item.status) {
    case "queued":
      return { dot: "idle", label: "Queued", overlay: null };
    case "working":
      return { dot: "work", label: "Working", overlay: null };
    case "waiting_approval":
      return { dot: "need", label: "Needs your OK", overlay: "Needs your OK" };
    case "waiting_person":
      return {
        dot: "need",
        label: "Question",
        overlay: item.question ? `Question for you · ${item.question}` : "Question for you",
      };
    case "done":
      return { dot: "done", label: "Sent", overlay: null };
    case "skipped": {
      const label =
        item.reason_code === "approval_rejected"
          ? "Skipped by you"
          : item.reason_code === "already_done"
            ? "Already sent"
            : "Skipped by your rules";
      return { dot: "off", label, overlay: null };
    }
    case "deferred":
      return { dot: "off", label: "Not reached today", overlay: null };
    case "failed":
      return { dot: "err", label: "Failed", overlay: null };
    case "retries_exhausted":
      return { dot: "err", label: "Gave up", overlay: null };
    case "stopped":
      return { dot: "off", label: "Stopped", overlay: null };
  }
}

export function runCounts(items: RunItemView[]): RunCounts {
  let sent = 0;
  let need = 0;
  for (const item of items) {
    if (item.status === "done") sent += 1;
    else if (item.status === "waiting_approval" || item.status === "waiting_person") need += 1;
  }
  return { total: items.length, sent, need };
}

/** The one line a collapsed playbook shows (design §3.3.2). */
export function sectionLine(run: { status: RunStatus } | null, counts: RunCounts): { text: string; need: boolean } {
  if (counts.need > 0) return { text: `${counts.need} need you`, need: true };
  if (!run) return { text: "Not run yet", need: false };
  switch (run.status) {
    case "running":
    case "discovering":
      return { text: `Running · ${counts.sent} of ${counts.total} sent`, need: false };
    case "paused":
      return { text: "Paused", need: false };
    case "stopped":
      return { text: "Stopped", need: false };
    case "failed":
      return { text: "Failed", need: false };
    case "done":
      return { text: `${counts.sent} sent`, need: false };
  }
}

/** Whether a playbook starts open: while its run is active, or when it needs the person. */
export function defaultOpen(run: { status: RunStatus } | null, counts: RunCounts): boolean {
  if (counts.need > 0) return true;
  return run?.status === "running" || run?.status === "discovering" || run?.status === "paused";
}
