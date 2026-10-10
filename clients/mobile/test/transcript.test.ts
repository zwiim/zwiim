import {
  clockTime,
  decodeServerFrame,
  initialSessionView,
  reduceSession,
  type HistoryTurn,
  type SessionEvent,
  type SessionFrame,
  type SessionView,
} from "@zwiim/sdk";
import { describe, expect, it } from "vitest";
import type { FakeChannel } from "../../sdk/test/fakes";
import { parseInline, parseMarkdown, spanText } from "../src/sessions/markdown";
import { clock, duration, historyBefore, nowState, transcriptRows, turnKey, turnRunning } from "../src/sessions/transcript";
import { waitFor, world } from "./support";

// ─── building views the way the SDK does ─────────────────────────────────

const snapshot = (status: "connected" | "prompting" = "connected"): SessionFrame => ({
  type: "snapshot",
  epoch: "e1",
  seq: 1,
  snapshot: {
    status,
    live_message: null,
    tool_calls: [],
    pending_request: null,
    pending_user_message: null,
    last_error: null,
    usage: null,
    options: null,
    commands: [],
  },
});

let seq = 1;
const ev = <E extends SessionEvent>(type: E["type"], payload: E["payload"]): SessionFrame => ({
  type: "event",
  event: { session_id: "s1", seq: ++seq, type, payload } as SessionEvent,
});

const user = (id: string, text: string) => ev("agent.user_message", { message_id: id, blocks: [{ type: "text", text }] });
const delta = (text: string) => ev("agent.message_delta", { text });
const thought = (text: string) => ev("agent.thought_delta", { text });
// As the SDK hands them over: every spelling of running decoded to `in_progress`.
const tool = (id: string, title: string, kind: string, status = "in_progress", content: unknown = null) =>
  ev("agent.tool_call", { tool_call_id: id, title, kind, status, content });
const toolDone = (id: string, status = "completed", content: unknown = null) =>
  ev("agent.tool_call_update", { tool_call_id: id, title: null, status, content });
const done = (stop_reason: string | null = "end_turn") => ev("agent.turn_complete", { stop_reason });

/** Events are numbered as they are made, so frames reused later replay as duplicates. */
function view(...frames: SessionFrame[]): SessionView {
  return frames.reduce(reduceSession, reduceSession(initialSessionView("s1"), snapshot()));
}
const play = (v: SessionView, ...frames: SessionFrame[]) => frames.reduce(reduceSession, v);

const turn = (id: string, role: string, blocks: HistoryTurn["blocks"], at = "2026-09-26T10:00:00Z", end?: string): HistoryTurn => ({
  turn_id: id,
  role,
  blocks,
  timestamp: at,
  completed_at: end ?? null,
  duration_ms: null,
  model: null,
  usage: null,
} as unknown as HistoryTurn);

const history = (turns: HistoryTurn[]): SessionFrame => ({ type: "history", turns });
const types = (v: SessionView) => transcriptRows(v).map((r) => r.type);

// ─── the transcript ──────────────────────────────────────────────────────

describe("transcript", () => {
  it("renders history: messages, thoughts, tool calls with their results, and how the turn ended", () => {
    const v = view(
      history([
        turn("u1", "user", [{ type: "text", text: "Fix the build" }], "2026-09-26T10:00:00Z"),
        turn(
          "a1",
          "assistant",
          [
            { type: "thought", text: "Look at the error first." },
            { type: "tool_call", tool_call_id: "t1", title: "`cargo build`", input_preview: "cargo build", status: null },
            { type: "tool_result", tool_call_id: "t1", output_preview: "error[E0425]", is_error: true },
            { type: "text", text: "Fixed the import." },
          ],
          "2026-09-26T10:00:05Z",
          "2026-09-26T10:03:17Z",
        ),
      ]),
    );
    const rows = transcriptRows(v);
    expect(rows.map((r) => r.type)).toEqual(["user", "thought", "tool", "agent", "footer"]);
    const t = rows[2]!;
    expect(t.type === "tool" && t.tool).toMatchObject({ verb: null, target: "cargo build", state: "failed", output: "error[E0425]" });
    expect(rows.at(-1)).toMatchObject({ text: `3m 17s · ${clockTime("2026-09-26T10:03:17Z")}`, stopped: false, files: [] });
  });

  it("reads a transcript's tool names and JSON input as a verb and a target", () => {
    const v = view(
      history([
        turn("u1", "user", [{ type: "text", text: "Test it" }]),
        turn("a1", "assistant", [
          { type: "tool_call", tool_call_id: "t1", title: "Bash", input_preview: '{"command":"cargo test","description":"Run tests"}', status: null },
          { type: "tool_result", tool_call_id: "t1", output_preview: "ok", is_error: false },
          { type: "tool_call", tool_call_id: "t2", title: "Read", input_preview: '{"file_path":"src/lib.rs"}', status: null },
          { type: "tool_call", tool_call_id: "t3", title: "TodoWrite", input_preview: '{"todos":[]}', status: null },
          { type: "tool_call", tool_call_id: "t4", title: "ExitPlanMode", input_preview: '{"plan":"Do it"}', status: null },
        ]),
      ]),
    );
    const tools = transcriptRows(v).flatMap((r) => (r.type === "tools" ? r.tools : r.type === "tool" ? [r.tool] : []));
    expect(tools.map((t) => [t.verb, t.target, t.state, t.input])).toEqual([
      ["Run", "cargo test", "done", "cargo test"],
      ["Read", "src/lib.rs", "idle", '{\n  "file_path": "src/lib.rs"\n}'],
      [null, "TodoWrite", "idle", '{\n  "todos": []\n}'],
      // Its verb says it all, as it reads live.
      [null, "Exited Plan Mode", "idle", '{\n  "plan": "Do it"\n}'],
    ]);
  });

  it("streams the live turn, with only its last block still writing", () => {
    const v = view(user("m1", "Run the tests"), thought("Plan"), tool("t1", "cargo test", "execute"), delta("Running"));
    const rows = transcriptRows(v);
    expect(rows.map((r) => [r.type, "streaming" in r ? r.streaming : null])).toEqual([
      ["user", null],
      ["thought", false],
      ["tool", null],
      ["agent", true],
    ]);
    const t = rows[2]!;
    expect(t.type === "tool" && t.tool).toMatchObject({ verb: "Run", target: "cargo test", state: "running" });
  });

  it("folds three or more tool calls in a row", () => {
    const v = view(
      user("m1", "Look around"),
      tool("t1", "Read src/a.ts", "read", "completed"),
      tool("t2", "src/b.ts", "read", "completed"),
      tool("t3", "grep foo", "search", "failed"),
      delta("Found it"),
      tool("t4", "src/c.ts", "edit", "completed", "--- a/c\n+++ b/c\n-old\n+new\n+more"),
    );
    const rows = transcriptRows(v);
    expect(rows.map((r) => r.type)).toEqual(["user", "tools", "agent", "tool"]);
    const group = rows[1]!;
    expect(group.type === "tools" && group.tools.map((t) => [t.verb, t.target, t.state])).toEqual([
      [null, "Read src/a.ts", "done"],
      ["Read", "src/b.ts", "done"],
      ["Search", "grep foo", "failed"],
    ]);
    const edit = rows[3]!;
    expect(edit.type === "tool" && [edit.tool.added, edit.tool.removed]).toEqual([2, 1]);
  });

  it("says a stopped turn stopped before the history has it, and keeps saying so after", () => {
    const base = view(user("m1", "Go"), tool("t1", "ls", "execute"), toolDone("t1"), delta("Done."));
    // A finished turn gets its row once the history has it.
    expect(types(play(base, done()))).not.toContain("footer");
    const stopped = play(base, done("cancelled"));
    expect(transcriptRows(stopped).at(-1)).toMatchObject({ type: "footer", text: "Stopped", stopped: true });
    // No footer while it runs or waits on the user.
    expect(types(base)).not.toContain("footer");
    const after = play(
      stopped,
      history([
        turn("u1", "user", [{ type: "text", text: "Go" }], "2026-09-26T10:00:00Z"),
        turn("a1", "assistant", [{ type: "text", text: "Done." }], "2026-09-26T10:00:01Z", "2026-09-26T10:00:42Z"),
      ]),
    );
    expect(transcriptRows(after).at(-1)).toMatchObject({ text: `Stopped · 42s · ${clockTime("2026-09-26T10:00:42Z")}`, stopped: true });
  });

  it("ends every answered prompt with its time and the files it changed", () => {
    const edit = (id: string, path: string) => ({
      type: "tool_call" as const,
      tool_call_id: id,
      title: "Edit",
      input_preview: JSON.stringify({ file_path: path, old_string: "a", new_string: "b\nc" }),
      status: null,
    });
    const v = view(
      history([
        turn("u1", "user", [{ type: "text", text: "One" }], "2026-09-26T10:00:00Z"),
        turn("a1", "assistant", [edit("t1", "src/a.ts"), edit("t2", "src/b.ts"), edit("t3", "src/a.ts")], "2026-09-26T10:00:01Z", "2026-09-26T10:09:31Z"),
        turn("u2", "user", [{ type: "text", text: "Two" }], "2026-09-26T10:10:00Z"),
        turn("a2", "assistant", [{ type: "text", text: "Nothing to change." }], "2026-09-26T10:10:01Z", "2026-09-26T10:10:05Z"),
      ]),
    );
    const rows = transcriptRows(v);
    expect(rows.map((r) => r.type)).toEqual(["user", "tools", "footer", "user", "agent", "footer"]);
    expect(rows[2]).toMatchObject({
      text: `9m 31s · ${clockTime("2026-09-26T10:09:31Z")}`,
      files: [
        { path: "src/a.ts", added: 4, removed: 2 },
        { path: "src/b.ts", added: 2, removed: 1 },
      ],
    });
    expect(rows[5]).toMatchObject({ text: `5s · ${clockTime("2026-09-26T10:10:05Z")}`, files: [] });
    // An edit row names its file, to open it.
    const tools = rows[1]!;
    expect(tools.type === "tools" && tools.tools.map((t) => t.path)).toEqual(["src/a.ts", "src/b.ts", "src/a.ts"]);
  });

  it("has no row for the prompt the agent is still answering", () => {
    const answered = history([
      turn("u1", "user", [{ type: "text", text: "Go" }], "2026-09-26T10:00:00Z"),
      turn("a1", "assistant", [{ type: "text", text: "Working on it" }], "2026-09-26T10:00:01Z", "2026-09-26T10:00:05Z"),
    ]);
    expect(types(view(answered))).toContain("footer");
    // Reattached mid-turn: the history holds the running prompt, the live turn has no prompt.
    const running = reduceSession(reduceSession(initialSessionView("s1"), snapshot("prompting")), answered);
    expect(types(running)).not.toContain("footer");
  });

  it("a tool still marked running after the turn ends is not shown as running", () => {
    const v = view(user("m1", "Go"), tool("t1", "sleep 100", "execute"), done("cancelled"));
    const t = transcriptRows(v)[1]!;
    expect(t.type === "tool" && t.tool.state).toBe("idle");
  });

  it("shows a history read mid-turn and the live turn once", () => {
    const v = view(
      history([
        turn("u0", "user", [{ type: "text", text: "Earlier" }]),
        turn("a0", "assistant", [{ type: "text", text: "Earlier answer" }]),
        // The agent's transcript already holds the running turn's start.
        turn("u1", "user", [{ type: "text", text: "Now" }]),
        turn("a1", "assistant", [{ type: "text", text: "Partial" }]),
      ]),
      user("m1", "Now"),
      delta("Partial and more"),
    );
    const rows = transcriptRows(v);
    expect(rows.map((r) => ("text" in r ? r.text : r.type))).toEqual([
      "Earlier",
      "Earlier answer",
      `0s · ${clockTime("2026-09-26T10:00:00Z")}`,
      "Now",
      "Partial and more",
    ]);
    expect(historyBefore(v.history!, v)).toHaveLength(2);
  });

  it("does not repeat anything when the stream replays after a reconnect", () => {
    const frames = [user("m1", "Go"), delta("Hello"), tool("t1", "ls", "execute")];
    const v = view(...frames);
    // The same events again (a replay from an older cursor) change nothing.
    const again = play(v, ...frames);
    expect(transcriptRows(again)).toEqual(transcriptRows(v));
    // A fresh snapshot replaces the live turn rather than adding to it.
    const snap: SessionFrame = {
      type: "snapshot",
      epoch: "e1",
      seq: 50,
      snapshot: {
        status: "prompting",
        live_message: { message_id: null, role: "assistant", content: [{ type: "text", text: "Hello" }, { type: "tool_call", tool_call_id: "t1" }] },
        tool_calls: [{ tool_call_id: "t1", title: "ls", kind: "execute", status: "in_progress", content: null }],
        pending_request: null,
        pending_user_message: { message_id: "m1", blocks: [{ type: "text", text: "Go" }] },
        last_error: null,
        usage: null,
        options: null,
        commands: [],
      },
    };
    expect(transcriptRows(play(v, snap)).map((r) => r.type)).toEqual(["user", "agent", "tool"]);
  });

  it("keeps a finished turn until history has it, then shows history alone", () => {
    const v = view(user("m1", "Go"), delta("Done."), done());
    expect(types(v)).toEqual(["user", "agent"]);
    const after = play(
      v,
      history([
        turn("u1", "user", [{ type: "text", text: "Go" }], "2026-09-26T10:00:00Z"),
        turn("a1", "assistant", [{ type: "text", text: "Done." }], "2026-09-26T10:00:01Z", "2026-09-26T10:00:42Z"),
      ]),
    );
    expect(transcriptRows(after).map((r) => ("text" in r ? r.text : r.type))).toEqual(["Go", "Done.", `42s · ${clockTime("2026-09-26T10:00:42Z")}`]);
  });

  it("shows the agent's last error", () => {
    const v = view(user("m1", "Go"), ev("agent.error", { message: "rate limited", code: 429 }), done("end_turn"));
    expect(types(v)).toContain("error");
  });

  it("shows agent images as rows after their tool call, from history and live", () => {
    const shot = { name: "ab.png", mime_type: "image/png", path: ".zwiim/images/ab.png" };
    const v = view(
      history([
        turn("a1", "assistant", [
          { type: "tool_call", tool_call_id: "r1", title: "Read", input_preview: JSON.stringify({ file_path: "/tmp/s.png" }), status: "completed" },
          { type: "tool_result", tool_call_id: "r1", output_preview: null, is_error: false, images: [shot] },
          { type: "agent_image", name: "cd.png", mime_type: "image/png", path: ".zwiim/images/cd.png" },
          { type: "text", text: "done" },
        ]),
      ]),
    );
    expect(types(v)).toEqual(["tool", "image", "image", "agent"]);
    const rows = transcriptRows(v);
    expect(rows[1]).toMatchObject({ type: "image", image: shot });

    const live = play(v, user("m1", "go"), ev("agent.tool_call", { tool_call_id: "g1", title: "Generate image", kind: "other", status: "completed", content: null, images: [shot] }));
    expect(types(live).slice(-2)).toEqual(["tool", "image"]);
  });

  it("shows a live message image as an image row between text", () => {
    const shot = { name: "ab.png", mime_type: "image/png", path: ".zwiim/images/ab.png" };
    const v = view(user("m1", "go"), delta("look:"), ev("agent.message_image", shot), delta(" done"));
    expect(types(v)).toEqual(["user", "agent", "image", "agent"]);
  });
});

// ─── the Now bar ─────────────────────────────────────────────────────────

describe("now bar", () => {
  it("says what the running turn does, from the SDK's events alone", () => {
    let v = view(user("m1", "Go"));
    expect(nowState(v, true)).toEqual({ type: "working", text: "Working" });
    v = play(v, thought("hmm"));
    expect(nowState(v, true)).toEqual({ type: "thinking" });
    v = play(v, tool("t1", "cargo test", "execute"));
    expect(nowState(v, true)).toEqual({ type: "working", text: "Running cargo test" });
    v = play(v, toolDone("t1"), delta("All green"));
    expect(nowState(v, true)).toEqual({ type: "working", text: "Responding" });
    expect(turnRunning(v)).toBe(true);
    expect(turnKey("s1", v)).toBe("s1:m1");
    v = play(v, done());
    expect(nowState(v, true)).toBeNull();
    expect(turnRunning(v)).toBe(false);
  });

  it.each(["inprogress", "in_progress"])("reads a running tool the same whichever way the host spells it (%s)", (wire) => {
    const started = view(user("m1", "Go"));
    const frame = decodeServerFrame(
      JSON.stringify({
        type: "event",
        subscription_id: "sub-1",
        event: {
          session_id: "s1",
          seq: ++seq,
          type: "agent.tool_call",
          payload: { tool_call_id: "w1", title: "cargo test", kind: "execute", status: wire, content: null },
        },
      }),
    );
    if (frame?.type !== "event") throw new Error("expected an event frame");
    const v = play(started, { type: "event", event: frame.event });
    expect(nowState(v, true)).toEqual({ type: "working", text: "Running cargo test" });
    expect(transcriptRows(v).find((r) => r.type === "tool")).toMatchObject({ tool: { state: "running" } });
  });

  it("waits on the user, reconnects, and reports a gone agent", () => {
    const v = view(user("m1", "Go"));
    expect(nowState(v, false)).toEqual({ type: "reconnecting" });
    const asking = play(
      v,
      ev("agent.request", {
        kind: "question",
        request_id: "r1",
        questions: [],
      } as never),
    );
    expect(nowState(asking, true)).toEqual({ type: "needs_you" });
    const gone = reduceSession(initialSessionView("s1"), {
      ...snapshot(),
      snapshot: { ...(snapshot() as Extract<SessionFrame, { type: "snapshot" }>).snapshot, status: "disconnected" },
    } as SessionFrame);
    expect(nowState(gone, true)).toEqual({ type: "disconnected" });
    expect(nowState(reduceSession(gone, { type: "detached", reason: "session_not_live" }), true)).toBeNull();
  });

  it("formats time in mono", () => {
    expect(clock(42_000)).toBe("0:42");
    expect(clock(12 * 60_000 + 5_000)).toBe("12:05");
    expect(clock(3_730_000)).toBe("1:02:10");
    expect(duration(192_000)).toBe("3m 12s");
  });
});

// ─── markdown ────────────────────────────────────────────────────────────

describe("markdown", () => {
  it("reads the blocks agents write", () => {
    expect(parseMarkdown("# Plan\n\nFirst line\nsecond line\n\n- one\n2. two\n> note\n```rust\nfn main() {}\n```\nafter")).toEqual([
      { type: "h", text: "Plan" },
      { type: "p", text: "First line\nsecond line" },
      { type: "li", marker: "•", text: "one" },
      { type: "li", marker: "2.", text: "two" },
      { type: "quote", text: "note" },
      { type: "code", text: "fn main() {}" },
      { type: "p", text: "after" },
    ]);
  });

  it("shows a fence still being written as code", () => {
    expect(parseMarkdown("Run:\n```\ncargo te")).toEqual([
      { type: "p", text: "Run:" },
      { type: "code", text: "cargo te" },
    ]);
  });

  it("reads inline code and bold", () => {
    expect(parseInline("Use `cargo test` **now**, ok")).toEqual([
      { text: "Use " },
      { text: "cargo test", code: true },
      { text: " " },
      { text: "now", bold: true },
      { text: ", ok" },
    ]);
    expect(parseInline("**`resume`** failed")).toEqual([{ text: "resume", code: true, bold: true }, { text: " failed" }]);
  });

  it("reads a line that is only an image as an image block, and links as spans", () => {
    expect(parseMarkdown("Here:\n![screen](.zwiim/images/a.png)\nafter")).toEqual([
      { type: "p", text: "Here:" },
      { type: "image", alt: "screen", src: ".zwiim/images/a.png" },
      { type: "p", text: "after" },
    ]);
    expect(parseInline("see [the report](out/r.pdf) now")).toEqual([
      { text: "see " },
      { text: "the report", href: "out/r.pdf" },
      { text: " now" },
    ]);
  });

  it("keeps the URL of a link that opens nothing in the app", () => {
    const [, link] = parseInline("see [docs](https://example.com/x)");
    expect(spanText(link!)).toBe("[docs](https://example.com/x)");
    expect(spanText({ text: "plain" })).toBe("plain");
  });
});

// ─── showing a session ───────────────────────────────────────────────────

type Sub = { subscription_id: string; session_id: string };
const subscribes = (ch: FakeChannel | undefined): Sub[] =>
  (ch?.sent.filter((m) => m.op === "session.subscribe") ?? []) as unknown as Sub[];
const detaches = (ch: FakeChannel | undefined) => ch?.sent.filter((m) => m.op === "session.detach") ?? [];

function hostWith(sessions: { session_id: string; live: boolean }[]) {
  const w = world({ onLan: false });
  const summaries = sessions.map((s) => ({
    session_id: s.session_id,
    workspace_id: "w1",
    title: s.session_id,
    agent: "claude_code",
    status: s.live ? "in_progress" : "completed",
    live: s.live,
    updated_at: "2026-09-26T10:00:00Z",
    created_at: "2026-09-26T10:00:00Z",
  }));
  w.host.backend.handlers["workspace.list"] = () => [
    { workspace_id: "w1", name: "site", alias: null, kind: "root", parent_workspace_id: null, branch: "main", path: "/site" },
  ];
  w.host.backend.handlers["session.list"] = () => summaries;
  const histories: string[] = [];
  w.host.backend.handlers["session.history"] = (p) => {
    histories.push((p as { session_id: string }).session_id);
    return { turns: [] };
  };
  w.host.backend.handlers["session.options"] = () => ({ options: null });
  const sends: unknown[] = [];
  w.host.backend.handlers["session.send"] = (p) => {
    sends.push(p);
    return {};
  };
  const c = w.connect({ mode: "cloud" });
  return { w, c, histories, sends };
}

describe("showing a session", () => {
  it("streams a finished session and loads its transcript while it is on screen", async () => {
    const { w, c, histories } = hostWith([{ session_id: "old", live: false }]);
    c.showSession("old"); // before the connection is up: kept until it is
    c.connect();
    await waitFor(() => subscribes(w.host.backend.channels.at(-1)).length === 1);
    expect(subscribes(w.host.backend.channel)[0]!.session_id).toBe("old");
    await waitFor(() => histories.includes("old"));
    await waitFor(() => c.store.getState().sessions.old?.history !== null && c.store.getState().sessions.old !== undefined);

    c.hideSession("old");
    await waitFor(() => detaches(w.host.backend.channel).length === 1);
    expect(c.store.getState().sessions.old).toBeUndefined();
    c.close();
  });

  it("keeps following a live session after it is hidden", async () => {
    const { w, c } = hostWith([{ session_id: "live", live: true }]);
    c.connect();
    await waitFor(() => subscribes(w.host.backend.channels.at(-1)).length === 1);
    c.showSession("live");
    c.hideSession("live");
    await new Promise((r) => setTimeout(r, 20));
    expect(detaches(w.host.backend.channel)).toEqual([]);
    // Shown and followed at once: one stream, not two.
    expect(subscribes(w.host.backend.channel)).toHaveLength(1);
    c.close();
  });

  it("sends a message with the phone's id, and refuses while offline", async () => {
    const { w, c, sends } = hostWith([{ session_id: "live", live: true }]);
    await expect(c.send("live", "hi", "id-1")).rejects.toMatchObject({ code: "network" });
    c.connect();
    await waitFor(() => subscribes(w.host.backend.channels.at(-1)).length === 1);
    await c.send("live", "hi", "id-1");
    expect(sends).toEqual([{ session_id: "live", blocks: [{ type: "text", text: "hi" }], client_message_id: "id-1" }]);
    c.close();
  });
});

describe("files sent with a message", () => {
  const pdf = {
    type: "attachment" as const,
    attachment_id: "att-1",
    name: "report.pdf",
    mime_type: "application/pdf",
    size: 2048,
    path: ".zwiim/attachments/att-1/report.pdf",
  };
  const sheet = { ...pdf, attachment_id: "att-2", name: "q3.xlsx", mime_type: "application/vnd.ms-excel", path: "x/q3.xlsx" };

  it("shows a history message's files, and one carrying only files", () => {
    const v = view(
      history([
        turn("u0", "user", [{ type: "text", text: "Summarize this" }, pdf]),
        turn("a0", "assistant", [{ type: "text", text: "Done" }]),
        turn("u1", "user", [{ ...sheet, size: undefined }]),
      ]),
    );
    const users = transcriptRows(v).filter((r) => r.type === "user");
    expect(users).toEqual([
      { type: "user", key: "u0", text: "Summarize this", files: [pdf] },
      { type: "user", key: "u1", text: "", files: [{ ...sheet, size: undefined }] },
    ]);
  });

  it("shows a live message's files", () => {
    const v = view(ev("agent.user_message", { message_id: "m1", blocks: [pdf] }), delta("Reading it"));
    expect(transcriptRows(v)[0]).toMatchObject({ type: "user", text: "", files: [pdf] });
    expect(types(v)).toEqual(["user", "agent"]);
  });

  it("drops the history's copy of a files-only turn shown live, and only that one", () => {
    const live = view(
      history([
        turn("u0", "user", [pdf]),
        turn("a0", "assistant", [{ type: "text", text: "Earlier" }]),
        turn("u1", "user", [sheet]),
      ]),
      ev("agent.user_message", { message_id: "m1", blocks: [sheet] }),
    );
    expect(historyBefore(live.history!, live).map((t) => t.turn_id)).toEqual(["u0", "a0"]);
    // Different files, same (empty) text: a different message.
    const other = view(history([turn("u0", "user", [pdf])]), ev("agent.user_message", { message_id: "m1", blocks: [sheet] }));
    expect(historyBefore(other.history!, other)).toHaveLength(1);
  });
});
