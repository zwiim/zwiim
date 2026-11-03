import { describe, expect, it } from "vitest";
import { derivePlanUi, meterTakesNowBar, type PlanUi } from "../src/state/plan";
import { initialSessionView, type SessionView } from "../src/state/session";
import type { Progress, SavedPlan, StepStatus } from "../src/types/protocol";

const steps = (...statuses: StepStatus[]) => statuses.map((status, i) => ({ title: `Step ${i + 1}`, status }));
const progress = (...statuses: StepStatus[]): Progress => ({
  steps: steps(...statuses),
  total: statuses.length,
  completed: statuses.filter((s) => s === "completed").length,
});
const saved = (resumable: boolean, ...statuses: StepStatus[]): SavedPlan => ({
  ...progress(...statuses),
  plan_id: "p1",
  agent: "claude_code",
  state: "interrupted",
  resumable,
});
const view = (patch: Partial<SessionView>): SessionView => ({ ...initialSessionView("s1"), ...patch });

describe("derivePlanUi", () => {
  it("is none without a plan", () => {
    expect(derivePlanUi(view({}))).toEqual({ kind: "none" });
  });

  it("is working while a turn runs with steps", () => {
    const p = progress("completed", "completed", "in_progress", "pending", "pending");
    expect(derivePlanUi(view({ agentStatus: "prompting", progress: p }))).toEqual({ kind: "working", progress: p, current: 2 });
  });

  it("has no current step when none is in progress", () => {
    const p = progress("completed", "pending");
    expect(derivePlanUi(view({ agentStatus: "prompting", progress: p }))).toEqual({ kind: "working", progress: p, current: null });
  });

  it("is none while a turn runs without steps", () => {
    expect(derivePlanUi(view({ agentStatus: "prompting", progress: progress() }))).toEqual({ kind: "none" });
    expect(derivePlanUi(view({ agentStatus: "prompting" }))).toEqual({ kind: "none" });
  });

  it("is stopped when history says resumable", () => {
    const plan = saved(true, "completed", "completed", "in_progress", "pending");
    expect(derivePlanUi(view({ agentStatus: "connected", savedPlan: plan }))).toEqual({ kind: "stopped", plan, stoppedAt: 2 });
  });

  it("is done when not resumable", () => {
    const plan = saved(false, "completed", "completed");
    expect(derivePlanUi(view({ savedPlan: plan, continuedIn: null }))).toEqual({ kind: "done" });
  });

  it("is continued when continued_in is set", () => {
    const plan = saved(false, "completed", "pending");
    expect(derivePlanUi(view({ savedPlan: plan, continuedIn: "s2" }))).toEqual({ kind: "continued", sessionId: "s2" });
  });

  it("ignores stopped while a turn runs", () => {
    const plan = saved(true, "completed", "pending");
    expect(derivePlanUi(view({ agentStatus: "prompting", savedPlan: plan })).kind).toBe("none");
    const p = progress("in_progress");
    expect(derivePlanUi(view({ agentStatus: "prompting", savedPlan: plan, progress: p })).kind).toBe("working");
  });
});

describe("meterTakesNowBar", () => {
  const working: PlanUi = { kind: "working", progress: progress("in_progress"), current: 0 };

  it("takes the Now bar only over the plain working line", () => {
    expect(meterTakesNowBar(working, "working")).toBe(true);
    for (const now of ["needs_you", "reconnecting", "disconnected", "waiting", "idle"] as const) {
      expect(meterTakesNowBar(working, now)).toBe(false);
    }
  });

  it("hides stopped while waiting", () => {
    const stopped: PlanUi = { kind: "stopped", plan: saved(true, "completed", "pending"), stoppedAt: 1 };
    expect(meterTakesNowBar(stopped, "waiting")).toBe(false);
    expect(meterTakesNowBar(stopped, "working")).toBe(false);
    expect(meterTakesNowBar({ kind: "none" }, "working")).toBe(false);
  });
});
