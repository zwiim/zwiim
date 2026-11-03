import { meterTakesNowBar, ZwiimClientError, type PlanUi, type ProgressStep, type SavedPlan, type StepStatus } from "@zwiim/sdk";
import { describe, expect, it } from "vitest";
import {
  ARCHIVED_REASON,
  CONTINUE_HEIGHT,
  CONTINUE_LABEL,
  CONTINUE_NOTE,
  ROOT_REASON,
  continuedInLabel,
  continueError,
  continueBlock,
  continueLabel,
  continuingLabel,
  doneCount,
  fromLabel,
  listedTitle,
  MAX_PIPS,
  type PipState,
  nowActivity,
  pipStates,
  pipWindow,
  planNowState,
  planSlot,
  runContinue,
  stoppedTitle,
} from "../src/sessions/plan";

const steps = (...s: StepStatus[]): ProgressStep[] => s.map((status, i) => ({ title: `Step ${i + 1}`, status }));

function working(...s: StepStatus[]): PlanUi {
  const list = steps(...s);
  const current = list.findIndex((x) => x.status === "in_progress");
  return {
    kind: "working",
    progress: { steps: list, completed: list.filter((x) => x.status === "completed").length, total: list.length },
    current: current < 0 ? null : current,
  };
}

function stopped(stoppedAt: number, ...s: StepStatus[]): PlanUi {
  const list = steps(...s);
  const plan: SavedPlan = {
    plan_id: "p1",
    agent: "claude_code",
    state: "interrupted",
    resumable: true,
    steps: list,
    completed: list.filter((x) => x.status === "completed").length,
    total: list.length,
  };
  return { kind: "stopped", plan, stoppedAt };
}

/** A host that records what Continue sent and re-reads. */
function fakeHost(result: () => Promise<string>) {
  const calls: { continued: [string, string | undefined][]; reloaded: string[] } = { continued: [], reloaded: [] };
  return {
    calls,
    host: {
      continuePlan: (id: string, agent?: string) => {
        calls.continued.push([id, agent]);
        return result();
      },
      reloadSession: (id: string) => {
        calls.reloaded.push(id);
      },
    },
  };
}

/** What the panel's state would hold after each step of a tap. */
function recorder() {
  const seen: { pending: boolean[]; error: (string | null)[]; gone: number } = { pending: [], error: [], gone: 0 };
  return {
    seen,
    set: {
      pending: (p: boolean) => seen.pending.push(p),
      error: (e: string | null) => seen.error.push(e),
      gone: () => seen.gone++,
    },
  };
}

const view = { session_id: "s1", agent: "claude_code" as const };
const failing = (code: ConstructorParameters<typeof ZwiimClientError>[0]) => () =>
  Promise.reject(new ZwiimClientError(code, "boom"));

describe("plan progress on the phone", () => {
  it("labels the stopped panel", () => {
    expect(stoppedTitle(3, "Run the LAN tests")).toBe("Stopped at task 3 · Run the LAN tests");
    expect(stoppedTitle(3, undefined)).toBe("Stopped at task 3");
    expect(doneCount(2, 5)).toBe("2 of 5 done");
  });

  it("names the button and note", () => {
    expect(CONTINUE_LABEL).toBe("Continue");
    expect(CONTINUE_NOTE).toBe("New session · gets the tasks and full history");
    expect(CONTINUE_HEIGHT).toBe(48);
  });

  it("gives pips their states", () => {
    expect(pipStates(working("completed", "completed", "in_progress", "pending", "pending"))).toEqual([
      "done",
      "done",
      "current",
      "pending",
      "pending",
    ]);
    expect(pipStates(stopped(2, "completed", "completed", "in_progress", "pending", "pending"))).toEqual([
      "done",
      "done",
      "stopped",
      "pending",
      "pending",
    ]);
    expect(pipStates({ kind: "none" })).toEqual([]);
  });

  it("omits agent for the session's own agent", async () => {
    const { host, calls } = fakeHost(async () => "s2");
    const { set } = recorder();
    expect(await runContinue(host, view, "claude_code", set)).toBe("s2");
    expect(calls.continued).toEqual([["s1", undefined]]);
  });

  it("passes another agent", async () => {
    const { host, calls } = fakeHost(async () => "s2");
    const { set } = recorder();
    expect(await runContinue(host, view, "codex", set)).toBe("s2");
    expect(calls.continued).toEqual([["s1", "codex"]]);
  });

  it("holds pending after it works, and refuses a second tap while it is out", async () => {
    let release!: (id: string) => void;
    const op = new Promise<string>((r) => (release = r));
    const { host, calls } = fakeHost(() => op);
    const { seen, set } = recorder();
    const first = runContinue(host, view, "claude_code", set);
    expect(seen.pending).toEqual([true]);
    expect(await runContinue(host, view, "claude_code", set)).toBeNull();
    expect(calls.continued).toHaveLength(1);
    release("s2");
    expect(await first).toBe("s2");
    expect(seen.pending).toEqual([true]);
  });

  it("maps nothing_to_continue: reloads history, hides the panel, shows no error", async () => {
    const { host, calls } = fakeHost(failing("nothing_to_continue"));
    const { seen, set } = recorder();
    expect(await runContinue(host, view, "claude_code", set)).toBeNull();
    expect(calls.reloaded).toEqual(["s1"]);
    expect(seen.gone).toBe(1);
    expect(seen.error).toEqual([null]);
    expect(seen.pending).toEqual([true, false]);
  });

  it("maps turn_in_progress: reloads history and shows no error", async () => {
    const { host, calls } = fakeHost(failing("turn_in_progress"));
    const { seen, set } = recorder();
    expect(await runContinue(host, view, "claude_code", set)).toBeNull();
    expect(calls.reloaded).toEqual(["s1"]);
    expect(seen.gone).toBe(0);
    expect(seen.error).toEqual([null]);
  });

  it("maps root_workspace", async () => {
    const { host, calls } = fakeHost(failing("root_workspace"));
    const { seen, set } = recorder();
    expect(await runContinue(host, view, "claude_code", set)).toBeNull();
    expect(seen.error.at(-1)).toBe("Continue needs a worktree. Start a new session from a worktree.");
    expect(seen.error.at(-1)).toBe(ROOT_REASON);
    expect(seen.pending).toEqual([true, false]);
    expect(calls.reloaded).toEqual([]);
  });

  it("maps the other refusals", () => {
    expect(continueError(new ZwiimClientError("workspace_removed", "x"), "Mac")).toBe(
      "This worktree was archived. Its tasks can't be continued here.",
    );
    expect(continueError(new ZwiimClientError("unknown_outcome", "x"), "Mac")).toContain("may or may not");
    expect(continueError(new ZwiimClientError("network", "x"), "Mac")).toBe("Not connected to Mac. Try again when it's back.");
    expect(continueError(new ZwiimClientError("timeout", "x"), "Mac")).toBe("Not connected to Mac. Try again when it's back.");
    expect(continueError(new Error("x"), "Mac")).toBe("Couldn't continue. Try again.");
  });

  it("labels the links", () => {
    expect(continuingLabel("Claude Code", 2, 5)).toBe("Continuing Claude Code's tasks · 2 of 5 done");
    expect(continuingLabel("Claude Code", undefined, undefined)).toBe("Continuing Claude Code's tasks");
    expect(continuingLabel(null, undefined, undefined)).toBe("Continuing the tasks");
    expect(continuedInLabel("X")).toBe("Continued in X →");
    expect(continuedInLabel(null)).toBe("Continued in new session →");
  });

  it("names the button for another agent only", () => {
    expect(continueLabel(true, "Claude Code")).toBe("Continue");
    expect(continueLabel(false, "Codex")).toBe("Continue with Codex");
  });

  it("links 'From' only to a session the store lists", () => {
    const lists = { w1: [{ session_id: "a", title: "Fix LAN" }, { session_id: "b", title: "" }] };
    expect(listedTitle(lists, "a")).toBe("Fix LAN");
    expect(listedTitle(lists, "b")).toBeNull();
    expect(listedTitle(lists, "gone")).toBeUndefined();
    expect(fromLabel("Fix LAN")).toBe("From Fix LAN");
    expect(fromLabel(null)).toBe("From the previous session");
  });

  it("blocks Continue up front in the root and in an archived worktree", () => {
    expect(continueBlock({ kind: "root" })).toBe(ROOT_REASON);
    expect(continueBlock({ kind: "worktree", removed: true })).toBe(ARCHIVED_REASON);
    expect(continueBlock({ kind: "worktree", removed: false })).toBeNull();
    expect(continueBlock(null)).toBeNull();
  });

  it("keeps Review over the meter", () => {
    const ui = working("completed", "in_progress");
    expect(meterTakesNowBar(ui, "working")).toBe(true);
    expect(meterTakesNowBar(ui, "needs_you")).toBe(false);
  });

  it("reads the phone's Now line as the meter's", () => {
    expect(planNowState({ type: "working" })).toBe("working");
    expect(planNowState({ type: "thinking" })).toBe("working");
    expect(planNowState({ type: "needs_you" })).toBe("needs_you");
    expect(planNowState({ type: "reconnecting" })).toBe("reconnecting");
    expect(planNowState({ type: "disconnected" })).toBe("disconnected");
    expect(planNowState({ type: "waiting" })).toBe("waiting");
    expect(planNowState(null)).toBe("idle");
  });
});

describe("planSlot", () => {
  const on = { showsPlan: true, canContinue: true, cardShown: false, gone: null };
  const parked = stopped(2, "completed", "completed", "pending");
  const moved: PlanUi = { kind: "continued", sessionId: "s2" };

  it("shows the resume panel for a stopped plan", () => {
    expect(planSlot({ ...on, ui: parked })).toBe("resume");
  });

  it("shows nothing without the progress capability, whatever the plan", () => {
    expect(planSlot({ ...on, showsPlan: false, ui: parked })).toBe("none");
    expect(planSlot({ ...on, showsPlan: false, ui: moved })).toBe("none");
  });

  it("offers Continue only with the continue capability", () => {
    expect(planSlot({ ...on, canContinue: false, ui: parked })).toBe("none");
  });

  it("hides the panel while the can't-reach card shows", () => {
    expect(planSlot({ ...on, cardShown: true, ui: parked })).toBe("none");
  });

  it("hides the panel once its plan was continued from here", () => {
    expect(planSlot({ ...on, gone: "p1", ui: parked })).toBe("none");
    expect(planSlot({ ...on, gone: "other", ui: parked })).toBe("resume");
  });

  it("shows where a continued plan went, after the card and without the continue capability", () => {
    expect(planSlot({ ...on, ui: moved })).toBe("continued");
    expect(planSlot({ ...on, cardShown: true, ui: moved })).toBe("continued");
    expect(planSlot({ ...on, canContinue: false, ui: moved })).toBe("continued");
  });

  it("shows nothing for a plan that is working, done or absent", () => {
    expect(planSlot({ ...on, ui: working("completed", "in_progress") })).toBe("none");
    expect(planSlot({ ...on, ui: { kind: "done" } })).toBe("none");
    expect(planSlot({ ...on, ui: { kind: "none" } })).toBe("none");
  });
});

describe("pipWindow", () => {
  const many = (n: number, at: number, mark: "current" | "stopped" = "current") =>
    Array.from({ length: n }, (_, i) => (i < at ? "done" : i === at ? mark : "pending") as PipState);

  it("keeps a short plan whole", () => {
    expect(pipWindow(many(5, 2))).toHaveLength(5);
    expect(pipWindow(many(MAX_PIPS, 2))).toHaveLength(MAX_PIPS);
  });

  it("cuts a long plan to a window that holds the current step", () => {
    for (const at of [0, 3, 14, 29]) {
      const w = pipWindow(many(30, at));
      expect(w).toHaveLength(MAX_PIPS);
      expect(w).toContain("current");
    }
  });

  it("holds the stopped step, and the next pending one when nothing is current", () => {
    expect(pipWindow(many(30, 20, "stopped"))).toContain("stopped");
    const idle = many(30, 25).map((s) => (s === "current" ? "pending" : s));
    expect(pipWindow(idle)).toContain("pending");
  });
});

describe("nowActivity", () => {
  it("is what the Now bar says while the agent works", () => {
    expect(nowActivity({ type: "working", text: "Running npm test" })).toBe("Running npm test");
    expect(nowActivity({ type: "thinking" })).toBe("Thinking");
  });

  it("is absent otherwise", () => {
    expect(nowActivity(null)).toBeNull();
    expect(nowActivity({ type: "needs_you" })).toBeNull();
  });
});
