import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { HostController } from "../src/client/controller";
import { HostClient } from "../src/client/host-client";
import { FakeTransport, flush } from "./fakes";
import { MUTATING_OPS, type PrState } from "../src/types/protocol";

const snapshot = {
  status: "prompting" as const,
  live_message: null,
  tool_calls: [],
  pending_request: null,
  pending_user_message: null,
  last_error: null,
  usage: null,
  options: null,
  commands: [],
};

function setup() {
  const transport = new FakeTransport();
  const client = new HostClient({ transport, random: () => 0.5 });
  const controller = new HostController(client);
  return { transport, client, controller, store: controller.store };
}

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe("HostController", () => {
  it("takes host lists from the feed snapshot on every connect, without list calls", async () => {
    const t = setup();
    let listed = [{ workspace_id: "w1", name: "proj", alias: null, kind: "root", parent_workspace_id: null }];
    t.transport.handlers["workspace.list"] = () => listed;
    t.client.start();
    await flush();
    t.transport.channel.open();
    await flush();
    expect(t.store.getState().workspaces).toHaveLength(1);
    expect(t.store.getState().sessionsByWorkspace).toEqual({ w1: [] });
    expect(t.store.getState().host?.host_id).toBe("host-1");

    t.transport.channel.drop();
    listed = [...listed, { ...listed[0]!, workspace_id: "w2" }];
    await vi.advanceTimersByTimeAsync(500);
    t.transport.channel.open();
    await flush();
    expect(t.store.getState().workspaces?.map((w) => w.workspace_id)).toEqual(["w1", "w2"]);
    for (const op of ["workspace.list", "session.list", "terminal.list"] as const) {
      expect(t.transport.count(op)).toBe(0);
    }
  });

  it.each(["inprogress", "in_progress"])("keeps one tool status in a loaded transcript (%s)", async (wire) => {
    const t = setup();
    t.transport.handlers["session.history"] = () => ({
      session_id: "s1",
      turns: [
        {
          turn_id: "t",
          role: "assistant",
          timestamp: "2026-09-26T00:00:00Z",
          completed_at: null,
          duration_ms: null,
          model: null,
          usage: null,
          blocks: [{ type: "tool_call", tool_call_id: "c", title: "Bash", input_preview: null, status: wire }],
        },
      ],
    });
    t.client.start();
    await flush();
    t.transport.channel.open();
    await flush();
    t.controller.openSession("s1");
    await flush();
    const block = t.store.getState().sessions.s1?.history?.[0]?.blocks[0];
    expect(block?.type === "tool_call" && block.status).toBe("in_progress");
  });

  it("re-reads on reconnect a transcript no attach will report on", async () => {
    const t = setup();
    t.client.start();
    await flush();
    t.transport.channel.open();
    await flush();
    t.controller.openSession("s1");
    await flush();
    // The agent is gone: no replay or snapshot follows a reconnect.
    t.transport.channel.frame({ type: "detached", subscription_id: "sub-1", reason: "session_not_live" });
    await flush();
    const before = t.transport.count("session.history");
    t.transport.channel.drop();
    await vi.advanceTimersByTimeAsync(500);
    t.transport.channel.open();
    await flush();
    expect(t.transport.count("session.history")).toBeGreaterThan(before);
  });

  /** An open, attached session whose history was read at generation `generation`. */
  async function attachedAt(generation: number) {
    const t = setup();
    t.transport.handlers["session.history"] = (p: any) => ({ session_id: p.session_id, turns: [], history_generation: generation });
    t.client.start();
    await flush();
    t.transport.channel.open();
    await flush();
    t.controller.openSession("s1");
    await flush();
    t.transport.channel.frame({
      type: "snapshot",
      subscription_id: "sub-1",
      session_id: "s1",
      epoch: "e",
      snapshot: { ...snapshot, history_generation: generation },
      event_seq: 1,
    });
    await flush();
    expect(t.store.getState().sessions.s1!.historyGeneration).toBe(generation);
    expect(t.store.getState().sessions.s1!.historyStale).toBe(false);
    return t;
  }

  /** Drop the connection and come back; the Host answers the re-attach with `snap`. */
  async function reconnectWithSnapshot(t: ReturnType<typeof setup>, snap: Record<string, unknown>) {
    t.transport.channel.drop();
    await vi.advanceTimersByTimeAsync(500);
    t.transport.channel.open();
    await flush();
    t.transport.channel.frame({ type: "snapshot", subscription_id: "sub-1", session_id: "s1", epoch: "e", snapshot: snap as any, event_seq: 60 });
    await flush();
  }

  it("mid_turn_snapshot_does_not_refetch_history", async () => {
    const t = await attachedAt(7);
    const before = t.transport.count("session.history");
    // Reconnect mid-turn after a long sub-agent stream: the attach fell back
    // to a snapshot carrying the sub-agent's live blocks. No turn finished.
    await reconnectWithSnapshot(t, {
      ...snapshot,
      history_generation: 7,
      live_message: {
        message_id: null,
        role: "assistant",
        content: [
          { type: "tool_call", tool_call_id: "launch" },
          { type: "tool_call", tool_call_id: "child-read" },
        ],
        subagents: [{ parent_id: "launch", blocks: [{ type: "text", text: "reading part 29." }, { type: "tool_call", tool_call_id: "child-read" }] }],
      },
      tool_calls: [
        { tool_call_id: "launch", title: "Explore", kind: "other", status: "in_progress", content: null, subagent: { description: "Explore" } },
        { tool_call_id: "child-read", title: "Read a.rs", kind: "read", status: "completed", content: null, parent_id: "launch" },
      ],
    });
    expect(t.transport.count("session.history")).toBe(before);
    const view = t.store.getState().sessions.s1!;
    expect(view.historyStale).toBe(false);
    expect(view.live.subagents?.launch?.blocks).toEqual([
      { type: "text", text: "reading part 29." },
      { type: "tool_call", tool_call_id: "child-read" },
    ]);
    expect(view.live.message?.content).toEqual([{ type: "tool_call", tool_call_id: "launch" }]);
  });

  // The same reconnect for both sub-agent shapes the Host sends: Claude's
  // launch is the Agent tool call (toolu_ ids); Codex's is a synthesized
  // launch (opaque subagent- id) whose children are call_ ids.
  it.each([
    {
      agent: "claude",
      launch: { tool_call_id: "toolu_01Launch", title: "Explore the parser", kind: "other", subagent: { description: "Explore" } },
      child: { tool_call_id: "toolu_01ChildRead", title: "Read src/a.rs", kind: "read" },
    },
    {
      agent: "codex",
      launch: { tool_call_id: "subagent-0123456789abcdef", title: "reviewer", kind: "agent", subagent: { description: "reviewer" } },
      child: { tool_call_id: "call_ChildExec01", title: "Run cargo test", kind: "execute" },
    },
  ])("mid_turn_reconnect_restores_$agent subagents_without_history_refetch", async ({ launch, child }) => {
    const t = await attachedAt(7);
    const before = t.transport.count("session.history");
    await reconnectWithSnapshot(t, {
      ...snapshot,
      history_generation: 7,
      live_message: {
        message_id: null,
        role: "assistant",
        content: [
          { type: "text", text: "Delegating." },
          { type: "tool_call", tool_call_id: launch.tool_call_id },
          { type: "tool_call", tool_call_id: child.tool_call_id },
        ],
        subagents: [
          {
            parent_id: launch.tool_call_id,
            blocks: [
              { type: "thinking", text: "Checking the tests." },
              { type: "tool_call", tool_call_id: child.tool_call_id },
              { type: "text", text: "All green so far." },
            ],
          },
        ],
      },
      tool_calls: [
        { ...launch, status: "in_progress", content: null },
        { ...child, status: "completed", content: null, parent_id: launch.tool_call_id },
      ],
    });
    // Advance past any settle delay a stray read would wait on.
    await vi.advanceTimersByTimeAsync(5000);
    expect(t.transport.count("session.history")).toBe(before);
    const view = t.store.getState().sessions.s1!;
    expect(view.historyStale).toBe(false);
    expect(view.historyGeneration).toBe(7);
    expect(view.live.subagents?.[launch.tool_call_id]?.blocks).toEqual([
      { type: "thinking", text: "Checking the tests." },
      { type: "tool_call", tool_call_id: child.tool_call_id },
      { type: "text", text: "All green so far." },
    ]);
    expect(view.live.message?.content).toEqual([
      { type: "text", text: "Delegating." },
      { type: "tool_call", tool_call_id: launch.tool_call_id },
    ]);
  });

  /** A session.background event as the Host sends it, carrying the history generation. */
  const backgroundEvent = (seq: number, history_generation: number) => ({
    session_id: "s1",
    seq,
    type: "session.background" as const,
    payload: { outstanding: 1, tasks: [], waiting_since: "2026-10-19T10:00:00Z", history_generation },
  });

  it("re-reads history when background turns move the generation", async () => {
    const t = await attachedAt(7);
    const before = t.transport.count("session.history");
    // A task finished after the turn ended and its turns landed: the Host
    // bumped the generation with no turn_complete.
    t.transport.handlers["session.history"] = (p: any) => ({ session_id: p.session_id, turns: [], history_generation: 8 });
    t.transport.channel.frame({ type: "event", subscription_id: "sub-1", event: backgroundEvent(2, 8) });
    await flush();
    expect(t.transport.count("session.history")).toBe(before + 1);
    let view = t.store.getState().sessions.s1!;
    expect(view.historyGeneration).toBe(8);
    expect(view.historyStale).toBe(false);
    expect(view.background).toEqual({ outstanding: 1, tasks: [], waiting_since: "2026-10-19T10:00:00Z" });

    // Background news at the generation on screen reads nothing.
    t.transport.channel.frame({ type: "event", subscription_id: "sub-1", event: backgroundEvent(3, 8) });
    await flush();
    expect(t.transport.count("session.history")).toBe(before + 1);
    view = t.store.getState().sessions.s1!;
    expect(view.historyStale).toBe(false);
  });

  it("re-reads history when a replay carries a moved generation", async () => {
    const t = await attachedAt(7);
    const before = t.transport.count("session.history");
    t.transport.handlers["session.history"] = (p: any) => ({ session_id: p.session_id, turns: [], history_generation: 8 });
    t.transport.channel.drop();
    await vi.advanceTimersByTimeAsync(500);
    t.transport.channel.open();
    await flush();
    t.transport.channel.frame({
      type: "replay",
      subscription_id: "sub-1",
      session_id: "s1",
      epoch: "e",
      events: [backgroundEvent(2, 8)],
      high_water_seq: 2,
    });
    await flush();
    expect(t.transport.count("session.history")).toBe(before + 1);
    expect(t.store.getState().sessions.s1!.historyGeneration).toBe(8);
  });

  it("a reconnect after background turns landed re-reads history", async () => {
    const t = await attachedAt(7);
    const before = t.transport.count("session.history");
    // While away, a task's turns landed out of turn; the snapshot carries
    // the bumped generation (the turn itself ended before the drop).
    t.transport.handlers["session.history"] = (p: any) => ({ session_id: p.session_id, turns: [], history_generation: 8 });
    await reconnectWithSnapshot(t, {
      ...snapshot,
      status: "connected",
      history_generation: 8,
      background: { outstanding: 1, tasks: [], waiting_since: "2026-10-19T10:00:00Z" },
    });
    expect(t.transport.count("session.history")).toBe(before + 1);
    const view = t.store.getState().sessions.s1!;
    expect(view.historyGeneration).toBe(8);
    expect(view.historyStale).toBe(false);
  });

  it("completed_turn_snapshot_refetches", async () => {
    const t = await attachedAt(7);
    const before = t.transport.count("session.history");
    t.transport.handlers["session.history"] = (p: any) => ({ session_id: p.session_id, turns: [], history_generation: 8 });
    await reconnectWithSnapshot(t, { ...snapshot, status: "connected", history_generation: 8 });
    expect(t.transport.count("session.history")).toBe(before + 1);
    const view = t.store.getState().sessions.s1!;
    expect(view.historyGeneration).toBe(8);
    expect(view.historyStale).toBe(false);
  });

  it("re-reads history after a chat rewind's snapshot", async () => {
    const t = await attachedAt(7);
    const before = t.transport.count("session.history");
    // The rewind forks the agent session; the Host bumps the generation and
    // the client gets a snapshot on the same connection.
    t.transport.channel.frame({
      type: "snapshot",
      subscription_id: "sub-1",
      session_id: "s1",
      epoch: "e2",
      snapshot: { ...snapshot, status: "connected", history_generation: 9 },
      event_seq: 1,
    });
    await flush();
    expect(t.transport.count("session.history")).toBe(before + 1);
  });

  it("re-reads history on every snapshot from a Host that predates generations", async () => {
    const t = await attachedAt(7);
    const before = t.transport.count("session.history");
    await reconnectWithSnapshot(t, { ...snapshot });
    expect(t.transport.count("session.history")).toBe(before + 1);
  });

  it("loads history on open and again after a turn completes", async () => {
    const t = setup();
    t.client.start();
    await flush();
    t.transport.channel.open();
    await flush();
    t.controller.openSession("s1");
    await flush();
    const ch = t.transport.channel;
    ch.frame({ type: "snapshot", subscription_id: "sub-1", session_id: "s1", epoch: "e", snapshot, event_seq: 1 });
    await flush();
    ch.frame({
      type: "event",
      subscription_id: "sub-1",
      event: { session_id: "s1", seq: 2, type: "agent.turn_complete", payload: { stop_reason: "end_turn" } },
    });
    await flush();
    // open + snapshot + turn_complete (the first two may coalesce)
    expect(t.transport.count("session.history")).toBeGreaterThanOrEqual(2);
    const view = t.store.getState().sessions.s1!;
    expect(view.historyStale).toBe(false);
    expect(view.lastStopReason).toBe("end_turn");
  });

  it("does not re-read a session being deleted, only one whose delete failed", async () => {
    const t = setup();
    let busy = false;
    t.transport.handlers["session.delete"] = () => {
      // The host stops the agent before the session is gone.
      t.transport.channel.frame({ type: "detached", subscription_id: "sub-1", reason: "session_not_live" });
      if (busy) throw new Error("turn_in_progress");
      return {};
    };
    t.client.start();
    await flush();
    t.transport.channel.open();
    await flush();
    t.controller.openSession("s1");
    await flush();
    t.transport.channel.frame({ type: "snapshot", subscription_id: "sub-1", session_id: "s1", epoch: "e", snapshot, event_seq: 1 });
    await flush();
    busy = true;
    const before = t.transport.count("session.history");
    await t.controller.deleteSession("s1").catch(() => {});
    await flush();
    expect(t.transport.count("session.history")).toBe(before + 1);
    busy = false;
    await t.controller.deleteSession("s1");
    await flush();
    expect(t.transport.count("session.history")).toBe(before + 1);
    expect(t.transport.count("session.options")).toBe(1);
  });

  it("keeps a finished turn on screen until the agent's transcript includes it", async () => {
    const t = setup();
    const user = { turn_id: "u", role: "user", blocks: [{ type: "text", text: "hi" }], timestamp: "", completed_at: null, duration_ms: null, model: null, usage: null };
    const reply = { ...user, turn_id: "a", role: "assistant", blocks: [{ type: "text", text: "hello" }] };
    let persisted: unknown[] = [];
    t.transport.handlers["session.history"] = () => ({ session_id: "s1", turns: persisted });
    t.client.start();
    await flush();
    t.transport.channel.open();
    await flush();
    t.controller.openSession("s1");
    await flush();
    const ch = t.transport.channel;
    ch.frame({ type: "snapshot", subscription_id: "sub-1", session_id: "s1", epoch: "e", snapshot, event_seq: 1 });
    await flush();
    const ev = (seq: number, type: string, payload: unknown) =>
      ch.frame({ type: "event", subscription_id: "sub-1", event: { session_id: "s1", seq, type, payload } as never });
    ev(2, "agent.user_message", { message_id: "m", blocks: [{ type: "text", text: "hi" }] });
    ev(3, "agent.message_delta", { text: "hello" });
    persisted = [user]; // the reply is not flushed yet
    ev(4, "agent.turn_complete", { stop_reason: "end_turn" });
    await flush();
    expect(t.store.getState().sessions.s1!.settled?.message?.content).toEqual([{ type: "text", text: "hello" }]);

    persisted = [user, reply];
    await vi.advanceTimersByTimeAsync(250);
    await flush();
    const view = t.store.getState().sessions.s1!;
    expect(view.settled).toBeNull();
    expect(view.history).toHaveLength(2);
  });

  it("gives up waiting and shows the host's history", async () => {
    const t = setup();
    t.transport.handlers["session.history"] = () => ({ session_id: "s1", turns: [] });
    t.client.start();
    await flush();
    t.transport.channel.open();
    await flush();
    t.controller.openSession("s1");
    await flush();
    const ch = t.transport.channel;
    ch.frame({ type: "snapshot", subscription_id: "sub-1", session_id: "s1", epoch: "e", snapshot, event_seq: 1 });
    ch.frame({ type: "event", subscription_id: "sub-1", event: { session_id: "s1", seq: 2, type: "agent.user_message", payload: { message_id: "m", blocks: [{ type: "text", text: "hi" }] } } });
    ch.frame({ type: "event", subscription_id: "sub-1", event: { session_id: "s1", seq: 3, type: "agent.turn_complete", payload: { stop_reason: "cancelled" } } });
    await flush();
    expect(t.store.getState().sessions.s1!.settled).not.toBeNull();
    await vi.advanceTimersByTimeAsync(4000);
    await flush();
    expect(t.store.getState().sessions.s1!.settled).toBeNull();
  });

  it("does not load history for sessions that are not open", async () => {
    const t = setup();
    t.controller.openSession("s1");
    t.controller.closeSession("s1");
    await flush();
    expect(t.store.getState().sessions.s1).toBeUndefined();
  });

  it("adds a workspace from the host's answer", async () => {
    const t = setup();
    t.transport.handlers["workspace.add"] = () => ({
      workspace_id: "w2",
      name: "extra",
      alias: null,
      kind: "root",
      parent_workspace_id: null,
    });
    await t.controller.addWorkspace("/tmp/extra");
    expect(t.store.getState().workspaces?.map((w) => w.workspace_id)).toEqual(["w2"]);
    expect(t.transport.requests.at(-1)?.options).toEqual({ mutating: true });
  });

  it("reverts a turn's files once, never retried", async () => {
    const t = setup();
    t.transport.handlers["session.revert"] = () => ({ files_restored: true, chat_rewound: false, head_moved: true });
    const out = await t.controller.revertSession("s1", "turn-2", { files: true, chat: true });
    expect(out).toEqual({ files_restored: true, chat_rewound: false, head_moved: true });
    expect(t.transport.requests.at(-1)?.params).toEqual({ session_id: "s1", turn_id: "turn-2", files: true, chat: true });
    expect(t.transport.requests.at(-1)?.options).toEqual({ mutating: true });
    expect(MUTATING_OPS.has("session.revert")).toBe(true);

    await t.controller.revertSession("s1", "turn-0", { files: true });
    expect(t.transport.requests.at(-1)?.params).toEqual({ session_id: "s1", turn_id: "turn-0", files: true, chat: false });
  });

  it("adds a workspace by its folder names under the host's home", async () => {
    const t = setup();
    t.transport.handlers["workspace.add"] = () => ({
      workspace_id: "w3",
      name: "zwiim",
      alias: null,
      kind: "root",
      parent_workspace_id: null,
      display_path: "~/Projects/zwiim",
    });
    const added = await t.controller.addWorkspaceAt(["Projects", "zwiim"]);
    expect(added.display_path).toBe("~/Projects/zwiim");
    expect(t.transport.requests.at(-1)?.params).toEqual({ home_path: ["Projects", "zwiim"] });
    expect(t.transport.requests.at(-1)?.options).toEqual({ mutating: true });
    expect(t.store.getState().workspaces?.[0]?.display_path).toBe("~/Projects/zwiim");
  });

  it("inspects a folder and checks a repository name", async () => {
    const t = setup();
    const inspection = { git: "none", branch: "main", suggested_name: "app", first_commit: null, github: null };
    t.transport.handlers["workspace.inspect"] = () => inspection;
    expect(await t.controller.inspectWorkspace("/Users/me/app")).toEqual(inspection);
    expect(t.transport.requests.at(-1)?.params).toEqual({ path: "/Users/me/app" });
    await t.controller.inspectWorkspaceAt(["app"]);
    expect(t.transport.requests.at(-1)?.params).toEqual({ home_path: ["app"] });
    expect(t.transport.requests.at(-1)?.options?.mutating).toBeFalsy();

    t.transport.handlers["github.repo_available"] = () => ({ available: false });
    expect(await t.controller.repoAvailable("octo", "app")).toBe(false);
    expect(t.transport.requests.at(-1)?.params).toEqual({ owner: "octo", name: "app" });
  });

  it("sends publish and allow_large only when set", async () => {
    const t = setup();
    const added = { workspace_id: "new", name: "app", alias: null, kind: "root", parent_workspace_id: null };
    t.transport.handlers["workspace.add"] = () => added;
    await t.controller.addWorkspace("/Users/me/app");
    expect(t.transport.requests.at(-1)?.params).toEqual({ path: "/Users/me/app" });
    expect(t.transport.requests.at(-1)?.options).toEqual({ mutating: true });
    await t.controller.addWorkspace("/Users/me/app", { allowLarge: false, publish: undefined });
    expect(t.transport.requests.at(-1)?.params).toEqual({ path: "/Users/me/app" });
    expect(t.transport.requests.at(-1)?.options).toEqual({ mutating: true });
    await t.controller.addWorkspaceAt(["app"], { allowLarge: true, publish: { owner: "octo", name: "app" } });
    expect(t.transport.requests.at(-1)?.params).toEqual({
      home_path: ["app"],
      allow_large: true,
      publish: { owner: "octo", name: "app" },
    });
    expect(t.transport.requests.at(-1)?.options).toEqual({ mutating: true, timeoutMs: 330_000 });
  });

  it("inspects and adds a listed root by workspace_id", async () => {
    const t = setup();
    const inspection = { git: "none", branch: "main", suggested_name: "app", first_commit: null, github: null };
    t.transport.handlers["workspace.inspect"] = () => inspection;
    expect(await t.controller.inspectRoot("w1")).toEqual(inspection);
    expect(t.transport.requests.at(-1)?.params).toEqual({ workspace_id: "w1" });
    expect(t.transport.requests.at(-1)?.options?.mutating).toBeFalsy();

    const added = { workspace_id: "w1", name: "app", alias: null, kind: "root", parent_workspace_id: null };
    t.transport.handlers["workspace.add"] = () => added;
    expect(await t.controller.addRoot("w1")).toEqual(added);
    expect(t.transport.requests.at(-1)?.params).toEqual({ workspace_id: "w1" });
    expect(t.transport.requests.at(-1)?.options).toEqual({ mutating: true });
    expect(t.store.getState().workspaces?.map((w) => w.workspace_id)).toEqual(["w1"]);

    await t.controller.addRoot("w1", { publish: { owner: "octo", name: "app" } });
    expect(t.transport.requests.at(-1)?.params).toEqual({
      workspace_id: "w1",
      publish: { owner: "octo", name: "app" },
    });
    expect(t.transport.requests.at(-1)?.options).toEqual({ mutating: true, timeoutMs: 330_000 });
  });

  it("merges the pull request at the head the user saw, never retried", async () => {
    expect(MUTATING_OPS.has("git.pr_merge")).toBe(true);
    expect(MUTATING_OPS.has("git.pr")).toBe(false);
    const t = setup();
    t.transport.handlers["git.pr_merge"] = () => ({ number: 7, method: "squash", merged_at: null });
    t.transport.handlers["git.pr"] = () => ({ pr: null, error: null });
    const pr = { number: 7, head_sha: "abc123" } as PrState;
    expect(await t.controller.mergePr("a", pr)).toEqual({ number: 7, method: "squash", merged_at: null });
    expect(t.transport.requests.at(-1)).toMatchObject({
      op: "git.pr_merge",
      params: { workspace_id: "a", number: 7, head_sha: "abc123" },
      options: { mutating: true },
    });
    await t.controller.mergePr("a", pr, "rebase");
    expect(t.transport.requests.at(-1)?.params).toMatchObject({ method: "rebase" });
    await t.controller.refreshPr("a");
    expect(t.transport.requests.at(-1)).toMatchObject({ op: "git.pr", params: { workspace_id: "a", refresh: true } });
  });

  it("never retries saving a file", () => {
    expect(MUTATING_OPS.has("files.write")).toBe(true);
    expect(MUTATING_OPS.has("files.read")).toBe(false);
  });

  it("browses without marking the read as mutating", () => {
    expect(MUTATING_OPS.has("workspace.browse")).toBe(false);
    expect(MUTATING_OPS.has("workspace.add")).toBe(true);
  });

  it("never retries creating a folder", async () => {
    expect(MUTATING_OPS.has("workspace.create_folder")).toBe(true);
    const t = setup();
    t.transport.handlers["workspace.create_folder"] = () => ({
      name: "new app",
      is_git_repo: false,
      is_workspace: false,
      is_readable: true,
    });
    const entry = await t.controller.client.request("workspace.create_folder", { path: ["Projects"], name: "new app" });
    expect(entry.name).toBe("new app");
    expect(t.transport.requests.at(-1)).toMatchObject({
      op: "workspace.create_folder",
      params: { path: ["Projects"], name: "new app" },
    });
  });
});

describe("GitHub clone", () => {
  it("lists a page with a long timeout", async () => {
    const t = setup();
    let seen: unknown;
    t.transport.handlers["github.repos"] = (params: unknown) => {
      seen = params;
      return { repos: [], next_page: null };
    };
    await expect(t.controller.githubRepos(2, true)).resolves.toEqual({ repos: [], next_page: null });
    expect(seen).toEqual({ page: 2, refresh: true });
  });

  it("starts and cancels a clone", async () => {
    const t = setup();
    t.transport.handlers["workspace.clone"] = () => ({ clone_id: "c_1" });
    t.transport.handlers["workspace.clone_cancel"] = () => ({ cancelled: true });
    await expect(t.controller.cloneWorkspace("octo", "app")).resolves.toEqual({ clone_id: "c_1" });
    await expect(t.controller.cancelClone("c_1")).resolves.toBe(true);
  });

  it("treats clone and cancel as mutating", () => {
    expect(MUTATING_OPS.has("workspace.clone")).toBe(true);
    expect(MUTATING_OPS.has("workspace.clone_cancel")).toBe(true);
    expect(MUTATING_OPS.has("github.repos")).toBe(false);
  });
});
