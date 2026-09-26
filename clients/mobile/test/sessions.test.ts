import { ZwiimClientError, type AgentRequest, type SessionSummary, type SessionView, type WorkspaceInfo } from "@zwiim/sdk";
import { describe, expect, it } from "vitest";
import type { FakeChannel } from "../../sdk/test/fakes";
import {
  diffCounts,
  diffLines,
  elapsed,
  orderOptions,
  pendingCount,
  questionAnswer,
  questionsAnswered,
  RECENT_LIMIT,
  requestAction,
  respondError,
  sessionSections,
  sessionState,
  togglePick,
} from "../src/sessions/model";
import { waitFor, world } from "./support";

const T0 = Date.parse("2026-09-26T10:00:00Z");
const iso = (minutes: number) => new Date(T0 + minutes * 60_000).toISOString();

const workspace = (id: string, name: string): WorkspaceInfo => ({
  workspace_id: id,
  name,
  alias: null,
  kind: "root" as WorkspaceInfo["kind"],
  parent_workspace_id: null,
  branch: "main",
  base_branch: null,
  base_commit: null,
  removed: false,
});

const summary = (id: string, over: Partial<SessionSummary> = {}): SessionSummary => ({
  session_id: id,
  workspace_id: "w1",
  title: `Session ${id}`,
  agent: "claude_code" as SessionSummary["agent"],
  status: "pending_review",
  live: false,
  created_at: iso(0),
  updated_at: iso(0),
  ...over,
});

const bash: AgentRequest = {
  kind: "permission",
  request_id: "r1",
  title: "Run npm test",
  tool: { tool_call_id: "t1", kind: "execute", text: null, diffs: [], command: "npm test" },
  plan_markdown: null,
  options: [
    { option_id: "allow", name: "Allow once", kind: "allow_once" },
    { option_id: "always", name: "Always allow", kind: "allow_always" },
    { option_id: "reject", name: "Reject", kind: "reject_once" },
  ],
} as AgentRequest;

const question: AgentRequest = {
  kind: "question",
  request_id: "q1",
  questions: [
    {
      question_id: "which",
      header: "Stack",
      question: "Which one?",
      multi_select: false,
      options: [
        { label: "A", description: "" },
        { label: "B", description: "" },
      ],
    },
    {
      question_id: "extras",
      header: "Extras",
      question: "Anything else?",
      multi_select: true,
      options: [
        { label: "lint", description: "" },
        { label: "tests", description: "" },
      ],
    },
  ],
} as AgentRequest;

const plan: AgentRequest = { kind: "plan_approval", request_id: "p1", plan_markdown: "1. Do it" } as AgentRequest;

/** A session view as far as the model reads it. */
const view = (over: Partial<SessionView> = {}): SessionView =>
  ({
    attach: "live",
    agentStatus: "connected",
    sessionStatus: null,
    pendingRequest: null,
    live: { toolCalls: {} },
    lastError: null,
    ...over,
  }) as unknown as SessionView;

describe("sessionState", () => {
  it("a pending request wins over everything", () => {
    expect(sessionState(summary("s", { live: true }), view({ pendingRequest: bash, agentStatus: "prompting" }))).toBe(
      "needs_you",
    );
  });

  it("follows the running agent while streaming", () => {
    const s = summary("s", { live: true, status: "in_progress" });
    expect(sessionState(s, view({ agentStatus: "prompting" }))).toBe("working");
    expect(sessionState(s, view({ agentStatus: "error" }))).toBe("error");
    expect(sessionState(s, view({ agentStatus: "disconnected" }))).toBe("disconnected");
  });

  it("falls back to the host's status", () => {
    expect(sessionState(summary("s", { status: "pending_review", live: true }))).toBe("ready");
    expect(sessionState(summary("s", { status: "pending_review" }))).toBe("finished");
    expect(sessionState(summary("s", { status: "in_progress", live: true }))).toBe("working");
    expect(sessionState(summary("s", { status: "in_progress" }))).toBe("idle");
    expect(sessionState(summary("s", { status: "completed" }))).toBe("completed");
    expect(sessionState(summary("s", { status: "cancelled" }))).toBe("cancelled");
  });
});

describe("sessionSections", () => {
  it("is not loaded before the host's lists arrive", () => {
    expect(sessionSections({ workspaces: null, sessionsByWorkspace: {}, sessions: {} }).loaded).toBe(false);
  });

  it("groups, orders and lists every session once", () => {
    const listed = [
      summary("old-wait", { live: true, updated_at: iso(1) }),
      summary("new-wait", { live: true, updated_at: iso(5) }),
      summary("busy", { live: true, status: "in_progress", updated_at: iso(3) }),
      summary("done", { status: "completed", updated_at: iso(2) }),
      summary("stopped", { status: "cancelled", updated_at: iso(4) }),
    ];
    const s = sessionSections({
      workspaces: [workspace("w1", "site")],
      // The same session listed twice (as a replay could) shows once.
      sessionsByWorkspace: { w1: listed, w2: [listed[3]!] },
      sessions: {
        "old-wait": view({ pendingRequest: bash }),
        "new-wait": view({ pendingRequest: { ...plan } }),
        busy: view({ agentStatus: "prompting", live: { toolCalls: { t: { title: "Editing pair.tsx" } } } as never }),
      },
    });
    expect(s.needsYou.map((i) => i.session_id)).toEqual(["old-wait", "new-wait"]);
    expect(s.working.map((i) => [i.session_id, i.activity])).toEqual([["busy", "Editing pair.tsx"]]);
    expect(s.recent.map((i) => i.session_id)).toEqual(["stopped", "done"]);
    expect(s.needsYou[0]).toMatchObject({ workspace: "site", request: bash, waitingSince: T0 + 60_000 });
  });

  it("orders NEEDS YOU by when the phone first saw each request", () => {
    const s = sessionSections(
      {
        workspaces: [],
        sessionsByWorkspace: { w1: [summary("a", { live: true }), summary("b", { live: true })] },
        sessions: { a: view({ pendingRequest: bash }), b: view({ pendingRequest: plan }) },
      },
      (id) => (id === "r1" ? 200 : 100),
    );
    expect(s.needsYou.map((i) => [i.session_id, i.waitingSince])).toEqual([
      ["b", 100],
      ["a", 200],
    ]);
  });

  it("caps RECENT and counts the rest", () => {
    const many = Array.from({ length: RECENT_LIMIT + 3 }, (_, i) => summary(`s${i}`, { updated_at: iso(i) }));
    const s = sessionSections({ workspaces: [], sessionsByWorkspace: { w1: many }, sessions: {} });
    expect(s.recent).toHaveLength(RECENT_LIMIT);
    expect(s.recent[0]!.session_id).toBe(`s${RECENT_LIMIT + 2}`);
    expect(s.more).toBe(3);
  });

  it("counts pending requests for the badge", () => {
    expect(pendingCount({ sessions: { a: view({ pendingRequest: bash }), b: view() } })).toBe(1);
  });
});

describe("requests", () => {
  it("answers a command in place, and sends everything else to the sheet", () => {
    expect(requestAction(bash)).toMatchObject({ type: "inline", command: "npm test" });
    expect(requestAction(question)).toEqual({ type: "review", label: "Answer" });
    expect(requestAction(plan)).toEqual({ type: "review", label: "Review plan" });
    const edit = {
      ...bash,
      tool: { ...(bash as { tool: object }).tool, command: null, diffs: [{ path: "a.ts", old_text: "a", new_text: "b" }] },
    } as AgentRequest;
    expect(requestAction(edit)).toEqual({ type: "review", label: "Review changes" });
  });

  it("puts rejecting first and allowing once last", () => {
    const options = (bash as Extract<AgentRequest, { kind: "permission" }>).options;
    expect(orderOptions(options).map((o) => o.kind)).toEqual(["reject_once", "allow_always", "allow_once"]);
  });

  it("builds a question's answer", () => {
    const qs = (question as Extract<AgentRequest, { kind: "question" }>).questions;
    let picked: Record<string, string[]> = {};
    expect(questionsAnswered(qs, picked)).toBe(false);
    picked = { which: togglePick(qs[0]!, [], "A") };
    picked.which = togglePick(qs[0]!, picked.which!, "B"); // single choice replaces
    picked.extras = togglePick(qs[1]!, togglePick(qs[1]!, [], "lint"), "tests");
    expect(questionsAnswered(qs, picked)).toBe(true);
    expect(questionAnswer(qs, picked, false)).toEqual({
      kind: "question",
      answer: {
        answers: [
          { question_id: "which", labels: ["B"] },
          { question_id: "extras", labels: ["lint", "tests"] },
        ],
      },
    });
    expect(questionAnswer(qs, {}, true)).toMatchObject({ kind: "question", answer: { declined: true } });
  });

  it("explains a failed answer", () => {
    expect(respondError(new ZwiimClientError("unknown_outcome", "x"), "Mac")).toMatch(/Not sure that went through/);
    expect(respondError(new ZwiimClientError("not_found", "x"), "Mac")).toMatch(/no longer waiting/);
    expect(respondError(new ZwiimClientError("network", "x"), "Mac")).toMatch(/Mac/);
  });

  it("shows only what changed in a diff", () => {
    const lines = diffLines("a\nb\nc\nd\ne\nf\ng\nh\n", "a\nb\nc\nd\nE\nf\ng\nh\n", 1);
    expect(diffCounts(lines)).toEqual({ added: 1, removed: 1 });
    expect(lines.length).toBeLessThanOrEqual(4);
    expect(diffCounts(diffLines(null, "x\ny"))).toEqual({ added: 2, removed: 0 });
  });

  it("says how long ago, briefly", () => {
    expect(elapsed(0, 500)).toBe("now");
    expect(elapsed(0, 42_000)).toBe("42s");
    expect(elapsed(0, 3 * 60_000)).toBe("3m");
    expect(elapsed(0, 2 * 3_600_000)).toBe("2h");
    expect(elapsed(0, 4 * 86_400_000)).toBe("4d");
  });
});

// ─── against a host ──────────────────────────────────────────────────────

const snapshotOf = (pending: AgentRequest | null) => ({
  status: pending ? ("prompting" as const) : ("connected" as const),
  live_message: null,
  tool_calls: [],
  pending_request: pending,
  pending_user_message: null,
  last_error: null,
  usage: null,
  options: null,
  commands: [],
});

type Sub = { subscription_id: string; session_id: string; since_seq?: number; epoch?: string };
const subscribes = (ch: FakeChannel | undefined): Sub[] =>
  (ch?.sent.filter((m) => m.op === "session.subscribe") ?? []) as unknown as Sub[];
/** The host's current event channel, if the phone has opened one. */
const latest = (w: ReturnType<typeof world>) => w.host.backend.channels.at(-1);

function hostWithSessions() {
  const w = world({ onLan: false });
  const sessions = [
    summary("live-1", { live: true, status: "in_progress", updated_at: iso(2) }),
    summary("old-1", { status: "completed", updated_at: iso(1) }),
  ];
  w.host.backend.handlers["workspace.list"] = () => [workspace("w1", "site")];
  w.host.backend.handlers["session.list"] = () => sessions;
  const answers: unknown[] = [];
  w.host.backend.handlers["session.respond"] = (p) => {
    answers.push(p);
    return {};
  };
  const c = w.connect({ mode: "cloud" });
  return { w, c, sessions, answers };
}

describe("following a host's sessions", () => {
  it("streams live sessions, lists a waiting request once, and drops it when the host resolves it", async () => {
    const { w, c, answers } = hostWithSessions();
    c.connect();
    const ch = () => w.host.backend.channel;
    await waitFor(() => subscribes(latest(w)).length === 1);
    const sub = subscribes(ch())[0]!;
    expect(sub.session_id).toBe("live-1"); // not the finished session

    const frame = { type: "snapshot" as const, subscription_id: sub.subscription_id, session_id: "live-1", epoch: "e1" };
    ch().frame({ ...frame, snapshot: snapshotOf(bash), event_seq: 3 });
    await waitFor(() => pendingCount(c.store.getState()) === 1);
    let s = sessionSections(c.store.getState(), (id) => c.requestSeenAt(id));
    expect(s.needsYou.map((i) => i.session_id)).toEqual(["live-1"]);
    expect(s.recent.map((i) => i.session_id)).toEqual(["old-1"]);
    expect(c.requestSeenAt("r1")).not.toBeNull();

    // A replay of the same state doesn't duplicate anything.
    ch().frame({ ...frame, snapshot: snapshotOf(bash), event_seq: 3 });
    await new Promise((r) => setTimeout(r, 10));
    s = sessionSections(c.store.getState());
    expect(s.needsYou).toHaveLength(1);
    expect(s.needsYou.length + s.working.length + s.recent.length).toBe(2);

    await c.request("session.respond", {
      session_id: "live-1",
      request_id: "r1",
      kind: "permission",
      answer: { option_id: "allow" },
    });
    expect(answers).toEqual([
      { session_id: "live-1", request_id: "r1", kind: "permission", answer: { option_id: "allow" } },
    ]);
    // Still waiting until the host says otherwise.
    expect(pendingCount(c.store.getState())).toBe(1);
    ch().frame({
      type: "event",
      subscription_id: sub.subscription_id,
      event: { session_id: "live-1", seq: 4, type: "agent.request_resolved", payload: { request_id: "r1" } } as never,
    });
    await waitFor(() => pendingCount(c.store.getState()) === 0);
    expect(c.requestSeenAt("r1")).toBeNull();
    c.close();
  });

  it("after a reconnect, resumes each stream from where it was, without duplicates", async () => {
    const { w, c } = hostWithSessions();
    c.connect();
    await waitFor(() => subscribes(latest(w)).length === 1);
    const first = w.host.backend.channel;
    const sub = subscribes(first)[0]!;
    first.frame({
      type: "snapshot",
      subscription_id: sub.subscription_id,
      session_id: "live-1",
      epoch: "e1",
      snapshot: snapshotOf(plan),
      event_seq: 7,
    });
    await waitFor(() => pendingCount(c.store.getState()) === 1);

    w.host.live[0]!.end(true);
    await waitFor(() => latest(w) !== first && subscribes(latest(w)).length === 1);
    const again = subscribes(w.host.backend.channel)[0]!;
    expect(again).toMatchObject({ session_id: "live-1", since_seq: 7, epoch: "e1" });
    // The last-known request stays listed while the stream resumes.
    const s = sessionSections(c.store.getState());
    expect(s.needsYou.map((i) => i.session_id)).toEqual(["live-1"]);
    expect(s.needsYou.length + s.working.length + s.recent.length).toBe(2);
    c.close();
  });

  it("lets go of a session whose agent stops", async () => {
    const { w, c, sessions } = hostWithSessions();
    c.connect();
    await waitFor(() => subscribes(latest(w)).length === 1);
    const ch = w.host.backend.channel;
    const sub = subscribes(ch)[0]!;
    ch.frame({
      type: "snapshot",
      subscription_id: sub.subscription_id,
      session_id: "live-1",
      epoch: "e1",
      snapshot: snapshotOf(question),
      event_seq: 1,
    });
    await waitFor(() => pendingCount(c.store.getState()) === 1);

    sessions[0] = { ...sessions[0]!, live: false, status: "cancelled", updated_at: iso(9) };
    w.host.backend.feedEvent("session.changed", { session: sessions[0] });
    await waitFor(() => ch.sent.some((m) => m.op === "session.detach"));
    await waitFor(() => pendingCount(c.store.getState()) === 0);
    const s = sessionSections(c.store.getState());
    expect(s.needsYou).toEqual([]);
    expect(s.recent.map((i) => [i.session_id, i.state])).toEqual([
      ["live-1", "cancelled"],
      ["old-1", "completed"],
    ]);
    c.close();
  });
});
