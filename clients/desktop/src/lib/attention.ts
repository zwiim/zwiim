// Telling the user an agent finished its turn: a system notification while
// this window is in the background, and a chime, each as chosen in settings.
// The host's session status says when: a turn that was working is now
// waiting for review (or was marked done). A stop or an error says nothing.

import { useEffect } from "react";
import type { AgentSummary, SessionSummary } from "@zwiim/sdk";
import type { NativeBridge } from "@/lib/bridge";
import { playChime } from "@/lib/chime";
import { useController } from "@/lib/host-context";
import { turnAlerts } from "@/lib/prefs";

type SessionIndex = ReadonlyMap<string, SessionSummary>;

/** Every listed session by id. */
export function indexSessions(byWorkspace: Readonly<Record<string, readonly SessionSummary[]>>): SessionIndex {
  const index = new Map<string, SessionSummary>();
  for (const sessions of Object.values(byWorkspace)) for (const s of sessions) index.set(s.session_id, s);
  return index;
}

/** The sessions whose turn finished between `before` and `after`. */
export function finishedTurns(before: SessionIndex, after: SessionIndex): SessionSummary[] {
  const done: SessionSummary[] = [];
  for (const [id, now] of after) {
    const was = before.get(id);
    if (was?.status === "in_progress" && (now.status === "pending_review" || now.status === "completed")) done.push(now);
  }
  return done;
}

/** The notification for one finished turn. */
export function turnNotice(session: SessionSummary, agents: readonly AgentSummary[] | null): { title: string; body: string } {
  const agent = agents?.find((a) => a.agent === session.agent)?.name ?? "The agent";
  return { title: `${agent} finished`, body: session.title?.trim() || "Ready for review" };
}

/** Watches the shown host's sessions for this window's lifetime. */
export function useTurnAlerts(bridge: NativeBridge): void {
  const { store } = useController();
  useEffect(() => {
    let listed = store.getState().sessionsByWorkspace;
    let seen = indexSessions(listed);
    return store.subscribe(() => {
      const state = store.getState();
      if (state.sessionsByWorkspace === listed) return;
      listed = state.sessionsByWorkspace;
      const now = indexSessions(listed);
      const done = finishedTurns(seen, now);
      seen = now;
      if (done.length === 0) return;
      const alerts = turnAlerts();
      if (alerts.sound) playChime();
      // The user is looking at this window: its session list already says so.
      if (!alerts.notify || !bridge.notifier || document.hasFocus()) return;
      for (const session of done) {
        const { title, body } = turnNotice(session, state.agents);
        bridge.notifier.notify(title, body).catch(() => {});
      }
    });
  }, [store, bridge]);
}
