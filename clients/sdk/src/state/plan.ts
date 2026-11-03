// What a session shows about its plan: derived from the session view alone.
// A running turn's steps are "working"; a saved plan the Host says can be
// continued is "stopped"; one that is not is "done"; a plan a new session
// took over is "continued". Clients show none of it unless the Host offers
// `agent.progress` / `session.continue_plan`.

import type { Progress, SavedPlan } from "../types/protocol";
import type { SessionView } from "./session";

export type PlanUi =
  | { kind: "none" }
  | { kind: "working"; progress: Progress; current: number | null } // index of the in_progress step
  | { kind: "stopped"; plan: SavedPlan; stoppedAt: number } // index of the first step not done
  | { kind: "continued"; sessionId: string }
  | { kind: "done" };

/** The Now line states the status meter shows. */
export type NowState = "working" | "needs_you" | "reconnecting" | "disconnected" | "waiting" | "idle";

export function derivePlanUi(view: SessionView): PlanUi {
  if (view.continuedIn !== null) return { kind: "continued", sessionId: view.continuedIn };
  // A turn in flight, as `isSessionWorking` reads it.
  if (view.agentStatus === "prompting") {
    const progress = view.progress;
    if (progress === null || progress.steps.length === 0) return { kind: "none" };
    const current = progress.steps.findIndex((s) => s.status === "in_progress");
    return { kind: "working", progress, current: current < 0 ? null : current };
  }
  const plan = view.savedPlan;
  if (plan === null) return { kind: "none" };
  if (!plan.resumable) return { kind: "done" };
  const stoppedAt = plan.steps.findIndex((s) => s.status !== "completed");
  return { kind: "stopped", plan, stoppedAt: stoppedAt < 0 ? plan.steps.length : stoppedAt };
}

/** Whether the plan's step bar takes the place of the Now bar: only over the plain working line. */
export function meterTakesNowBar(ui: PlanUi, now: NowState): boolean {
  return ui.kind === "working" && now === "working";
}
