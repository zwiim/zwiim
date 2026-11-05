import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  CLIENT_CAPABILITIES,
  clockTime,
  FORK_OLDER_TURN_NOTE,
  HostClient,
  HostController,
  ZwiimClientError,
  type AgentRequest,
  type ForkTarget,
  type HistoryTurn,
  type SessionSnapshot,
  type WorkspaceInfo,
} from "@zwiim/sdk";
import { FileLinksContext, linkText, Markdown, workspaceFile, type FileLinks } from "@/components/Markdown";
import { errorMessage, HostProvider } from "@/lib/host-context";
import { rememberExpandToolCalls } from "@/lib/prefs";
import { dropForkedDraft, forkedDraft } from "@/panels/forkedDraft";
import { RequestCard } from "@/panels/RequestCard";
import { SessionPanel } from "@/panels/SessionPanel";
import { FakeTransport, hostHello, tick } from "./fakes";

afterEach(() => {
  cleanup();
  window.localStorage.clear();
});

const turn = (turn_id: string, role: string, blocks: HistoryTurn["blocks"]): HistoryTurn => ({
  turn_id,
  role,
  blocks,
  timestamp: "",
  completed_at: null,
  duration_ms: null,
  model: null,
  usage: null,
});

const snapshot = (extra: Partial<SessionSnapshot> = {}): SessionSnapshot => ({
  status: "connected",
  live_message: null,
  tool_calls: [],
  pending_request: null,
  pending_user_message: null,
  last_error: null,
  usage: null,
  options: null,
  commands: [],
  ...extra,
});

async function openSession(t: FakeTransport, links?: FileLinks, onOpenSession?: (workspaceId: string, id: string) => void) {
  const client = new HostClient({ transport: t });
  const controller = new HostController(client);
  client.start();
  await tick();
  const panel = <SessionPanel workspaceId="w1" sessionId="s1" onSessionStarted={() => {}} onOpenSession={onOpenSession} />;
  render(
    <HostProvider value={controller}>
      {links ? <FileLinksContext.Provider value={links}>{panel}</FileLinksContext.Provider> : panel}
    </HostProvider>,
  );
  await tick();
  const frame = (f: object) => act(async () => t.frame({ subscription_id: "sub-1", session_id: "s1", ...f } as never));
  return { controller, frame };
}

describe("transcript", () => {
  it("renders markdown and code blocks, never raw HTML", async () => {
    const t = new FakeTransport();
    t.handlers["session.history"] = (p) => ({
      session_id: p.session_id,
      turns: [
        turn("t1", "assistant", [
          { type: "text", text: "Here is **bold**\n\n```rust\nfn main() {}\n```\n\n<img src=x onerror=alert(1)><script>bad()</script>" },
        ]),
      ],
    });
    const { controller } = await openSession(t);
    const md = screen.getByTestId("markdown");
    expect(md.querySelector("strong")?.textContent).toBe("bold");
    expect(md.querySelector("pre code")?.textContent).toContain("fn main() {}");
    expect(md.querySelector("pre code")?.getAttribute("data-language")).toBe("rust");
    expect(md.querySelector("script")).toBeNull();
    expect(md.querySelector("img")).toBeNull();
    controller.dispose();
  });

  it("shows a tool call as what it does and on what, its input and output on expand", async () => {
    const t = new FakeTransport();
    t.handlers["session.history"] = (p) => ({
      session_id: p.session_id,
      turns: [
        turn("t1", "assistant", [
          { type: "tool_call", tool_call_id: "c1", title: "Bash", input_preview: '{"command":"ls src","description":"List"}', status: null },
          { type: "tool_result", tool_call_id: "c1", output_preview: "main.rs", is_error: false },
          { type: "tool_call", tool_call_id: "c2", title: "Read", input_preview: "x.txt", status: "completed" },
          { type: "tool_result", tool_call_id: "c2", output_preview: "no such file", is_error: true },
          { type: "tool_call", tool_call_id: "c3", title: "ExitPlanMode", input_preview: '{"plan":"# Plan"}', status: null },
          {
            type: "tool_call",
            tool_call_id: "c4",
            title: "mcp__zwiim-mcp__update_progress",
            input_preview: '{"steps":[{"title":"Add the form","status":"in_progress"}]}',
            status: null,
          },
        ]),
      ],
    });
    const { controller } = await openSession(t);
    const calls = screen.getAllByTestId("tool-call");
    expect(calls).toHaveLength(4);
    // Named as Claude titles it live.
    expect(calls[2]!.textContent).toContain("Exited Plan Mode");
    // Zwiim's own tool reads by what it does, never by its MCP name.
    expect(calls[3]!.textContent).toContain("Updated tasks");
    expect(calls[3]!.textContent).not.toContain("zwiim-mcp");
    const row = within(calls[0]!).getByRole("button");
    expect(row.textContent).toContain("Run");
    expect(row.querySelector("code")?.textContent).toBe("ls src");
    expect(calls[0]!.textContent).toContain("done");
    expect(row.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(row);
    expect(row.getAttribute("aria-expanded")).toBe("true");
    expect(calls[0]!.textContent).toContain("main.rs");
    expect(calls[1]!.getAttribute("data-status")).toBe("failed");
    // The result is shown with its call, not again on its own.
    expect(screen.getAllByText("no such file")).toHaveLength(1);
    controller.dispose();
  });

  it("shows a huge tool output cut short, so it cannot stall the transcript", async () => {
    const t = new FakeTransport();
    const huge = "x".repeat(1_000_000);
    t.handlers["session.history"] = (p) => ({
      session_id: p.session_id,
      turns: [
        turn("t1", "assistant", [
          { type: "tool_call", tool_call_id: "c1", title: "Read", input_preview: "a.bin", status: "completed" },
          { type: "tool_result", tool_call_id: "c1", output_preview: huge, is_error: false },
          { type: "tool_call", tool_call_id: "c2", title: "Bash", input_preview: '{"command":"cat a.bin"}', status: "completed" },
          { type: "tool_result", tool_call_id: "c2", output_preview: huge, is_error: false },
        ]),
      ],
    });
    const { controller } = await openSession(t);
    for (const call of screen.getAllByTestId("tool-call")) {
      fireEvent.click(within(call).getAllByRole("button")[0]!);
      expect(call.textContent!.length).toBeLessThan(25_000);
      expect(call.textContent).toContain("980,000 more characters");
    }
    controller.dispose();
  });

  it("shows live tool calls as they run and finish, with edits as a diff", async () => {
    const t = new FakeTransport();
    const { controller, frame } = await openSession(t);
    await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot({ status: "prompting" }) });
    await frame({
      type: "event",
      event: { session_id: "s1", seq: 2, type: "agent.tool_call", payload: { tool_call_id: "k", title: "Edit a.txt", kind: "edit", status: "in_progress", content: null } },
    });
    expect(screen.getByTestId("tool-call").textContent).toContain("running");
    // The verb in the title reads once, the file as what it works on.
    expect(screen.getByTestId("tool-call").textContent).toBe("Edita.txtrunning");
    await frame({
      type: "event",
      event: { session_id: "s1", seq: 3, type: "agent.tool_call_update", payload: { tool_call_id: "k", title: null, status: "completed", content: "--- a.txt\n+++ a.txt\n-old\n+new\n" } },
    });
    const call = screen.getByTestId("tool-call");
    expect(call.textContent).toContain("done");
    expect(within(call).getByTestId("diff-counts").textContent).toBe("+1−1");
    // Nothing opens inside the transcript; the file shows its change in a card over it.
    expect(call.querySelector("[data-kind]")).toBeNull();
    fireEvent.click(within(call).getByRole("button", { name: "a.txt" }));
    const lines = [...screen.getByTestId("hover-card").querySelectorAll("[data-kind]")].map((d) => [d.getAttribute("data-kind"), d.textContent]);
    expect(lines).toEqual([
      ["del", "-old"],
      ["add", "+new"],
    ]);
    controller.dispose();
  });

  it("names Zwiim's own tool by what it does while it runs", async () => {
    const t = new FakeTransport();
    const { controller, frame } = await openSession(t);
    await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot({ status: "prompting" }) });
    await frame({
      type: "event",
      event: {
        session_id: "s1",
        seq: 2,
        type: "agent.tool_call",
        payload: { tool_call_id: "p", title: "mcp__zwiim-mcp__update_progress", kind: "other", status: "in_progress", content: null },
      },
    });
    expect(screen.getByTestId("tool-call").textContent).toBe("Updated tasksrunning");
    controller.dispose();
  });

  it("shows a command and its output in one card", async () => {
    const t = new FakeTransport();
    t.handlers["session.history"] = (p) => ({
      session_id: p.session_id,
      turns: [
        turn("t1", "assistant", [
          { type: "tool_call", tool_call_id: "c1", title: "Bash", input_preview: '{"command":"cd src && ls\\nwc -l a.rs","description":"List"}', status: null },
          { type: "tool_result", tool_call_id: "c1", output_preview: "a.rs\n3 a.rs", is_error: false },
        ]),
      ],
    });
    const { controller } = await openSession(t);
    const call = screen.getByTestId("tool-call");
    fireEvent.click(within(call).getByRole("button"));
    const pres = [...call.querySelectorAll("pre")].map((p) => p.textContent);
    expect(pres).toEqual(["$ cd src && ls\nwc -l a.rs", "a.rs\n3 a.rs"]);
    // No raw JSON.
    expect(call.textContent).not.toContain('"command"');
    controller.dispose();
  });

  it("shows an edit as its file and a numbered diff, not its JSON input", async () => {
    const t = new FakeTransport();
    const input = JSON.stringify({ file_path: "/w/src/app.ts", old_string: "a\nb", new_string: "a\nc", replace_all: false });
    t.handlers["session.history"] = (p) => ({
      session_id: p.session_id,
      turns: [
        turn("t1", "assistant", [
          { type: "tool_call", tool_call_id: "c1", title: "Edit", input_preview: input, status: null },
          {
            type: "tool_result",
            tool_call_id: "c1",
            output_preview: "--- a//w/src/app.ts\n+++ b//w/src/app.ts\n@@ -9,2 +9,2 @@\n a\n-b\n+c\n",
            is_error: false,
          },
          { type: "tool_call", tool_call_id: "c2", title: "Write", input_preview: JSON.stringify({ file_path: "/w/new.md", content: "x\ny\n" }), status: null },
          { type: "tool_result", tool_call_id: "c2", output_preview: "File created", is_error: false },
        ]),
      ],
    });
    const { controller } = await openSession(t);
    const [edit, write] = screen.getAllByTestId("tool-call");
    // Only the file opens the change; the row itself opens nothing.
    const file = within(edit!).getByRole("button");
    expect(file.textContent).toBe("app.ts");
    expect(file.getAttribute("title")).toBe("/w/src/app.ts");
    expect(within(edit!).getByTestId("diff-counts").textContent).toBe("+1−1");
    expect(file.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(file);
    expect(file.getAttribute("aria-expanded")).toBe("true");
    const diff = within(screen.getByTestId("hover-card")).getByTestId("diff");
    expect(diff.textContent).toContain("/w/src/app.ts");
    expect([...diff.querySelectorAll("[data-kind]")].map((d) => d.textContent)).toEqual(["99a", "10-b", "10+c"]);
    expect(edit!.textContent).not.toContain("old_string");
    // Without a diff from the agent, the change is read from the input.
    expect(within(write!).getByTestId("diff-counts").textContent).toBe("+2−0");
    fireEvent.click(file);
    expect(screen.queryByTestId("hover-card")).toBeNull();
    fireEvent.click(within(write!).getByRole("button"));
    expect([...screen.getByTestId("hover-card").querySelectorAll("[data-kind]")].map((d) => d.textContent)).toEqual(["+x", "+y"]);
    expect(write!.textContent).not.toContain("File created");
    controller.dispose();
  });

  it("shows an edit's change while the pointer rests on its file", async () => {
    const t = new FakeTransport();
    t.handlers["session.history"] = (p) => ({
      session_id: p.session_id,
      turns: [
        turn("t1", "assistant", [
          { type: "tool_call", tool_call_id: "c1", title: "Edit", input_preview: JSON.stringify({ file_path: "/w/a.ts", old_string: "a", new_string: "b" }), status: null },
        ]),
      ],
    });
    const { controller } = await openSession(t);
    const call = screen.getByTestId("tool-call");
    const file = within(call).getByRole("button", { name: "a.ts" });
    vi.useFakeTimers();
    try {
      // The rest of the row does nothing.
      fireEvent.mouseEnter(within(call).getByText("Edit"));
      act(() => void vi.advanceTimersByTime(200));
      expect(screen.queryByTestId("hover-card")).toBeNull();
      fireEvent.mouseEnter(file);
      expect(screen.queryByTestId("hover-card")).toBeNull();
      act(() => void vi.advanceTimersByTime(200));
      const card = screen.getByTestId("hover-card");
      expect(card.getAttribute("role")).toBe("tooltip");
      // The pointer can move from the file into the card.
      fireEvent.mouseLeave(file);
      fireEvent.mouseEnter(card);
      act(() => void vi.advanceTimersByTime(200));
      expect(screen.getByTestId("hover-card")).toBeTruthy();
      fireEvent.mouseLeave(card);
      act(() => void vi.advanceTimersByTime(200));
      expect(screen.queryByTestId("hover-card")).toBeNull();
    } finally {
      vi.useRealTimers();
    }
    controller.dispose();
  });

  it("previews thinking on its row and shows all of it on expand", async () => {
    const t = new FakeTransport();
    t.handlers["session.history"] = (p) => ({
      session_id: p.session_id,
      turns: [
        turn("t1", "assistant", [
          { type: "thought", text: "**Planning the fix**\n\nFirst read the file, then edit it." },
          { type: "thought", text: "" },
        ]),
      ],
    });
    const { controller } = await openSession(t);
    const [full, empty] = screen.getAllByTestId("thought");
    const row = within(full!).getByRole("button");
    expect(within(full!).getByTestId("thought-preview").textContent).toBe("Planning the fix");
    fireEvent.click(row);
    expect(row.getAttribute("aria-expanded")).toBe("true");
    expect(within(full!).queryByTestId("thought-preview")).toBeNull();
    expect(full!.querySelector("strong")?.textContent).toBe("Planning the fix");
    expect(full!.textContent).toContain("First read the file, then edit it.");
    // Redacted thinking has nothing to open.
    expect(within(empty!).queryByRole("button")).toBeNull();
    controller.dispose();
  });

  it("reads a Codex transcript's tools the same way", async () => {
    const t = new FakeTransport();
    const patch = [
      "*** Begin Patch",
      "*** Update File: /w/src/app.ts",
      "@@ export function main",
      " a",
      "-b",
      "+c",
      "*** Add File: /w/src/new.ts",
      "+x",
      "+y",
      "*** End Patch",
    ].join("\n");
    t.handlers["session.history"] = (p) => ({
      session_id: p.session_id,
      turns: [
        turn("t1", "assistant", [
          { type: "tool_call", tool_call_id: "c1", title: "exec_command", input_preview: "cd src && ls\nwc -l a.rs", status: null },
          { type: "tool_result", tool_call_id: "c1", output_preview: "a.rs", is_error: false },
          { type: "tool_call", tool_call_id: "c2", title: "apply_patch", input_preview: patch, status: null },
          { type: "tool_result", tool_call_id: "c2", output_preview: "{}", is_error: false },
          {
            type: "tool_call",
            tool_call_id: "c3",
            title: "apply_patch",
            input_preview: "*** Begin Patch\n*** Add File: /w/b.md\n+hi\n*** End Patch",
            status: null,
          },
          {
            type: "tool_call",
            tool_call_id: "c4",
            title: "write_stdin",
            input_preview: JSON.stringify({ chars: "q", session_command: "npm run dev", session_id: 4 }),
            status: null,
          },
          {
            type: "tool_call",
            tool_call_id: "c5",
            title: "request_user_input",
            input_preview: JSON.stringify({ questions: [{ id: "a", header: "Mode", question: "Which mode?" }] }),
            status: null,
          },
          {
            type: "tool_call",
            tool_call_id: "c6",
            title: "codex_script",
            input_preview: JSON.stringify({ call_count: 2, source: "const a = 1;\nreturn a;" }),
            status: null,
          },
        ]),
      ],
    });
    const { controller } = await openSession(t);
    const [run, edit, write, input, ask, script] = screen.getAllByTestId("tool-call");
    const row = (el: HTMLElement) => [...el.querySelectorAll("span, code, button")].map((e) => e.textContent).join("|");

    fireEvent.click(within(run!).getByRole("button"));
    expect([...run!.querySelectorAll("pre")].map((p) => p.textContent)).toEqual(["$ cd src && ls\nwc -l a.rs", "a.rs"]);

    // A patch reads as its files and their changes.
    expect(edit!.textContent).toContain("Edit");
    const files = within(edit!).getByRole("button");
    expect(files.textContent).toBe("2 files");
    expect(files.getAttribute("title")).toBe("/w/src/app.ts\n/w/src/new.ts");
    expect(within(edit!).getByTestId("diff-counts").textContent).toBe("+3−1");
    fireEvent.click(files);
    const diffs = within(screen.getByTestId("hover-card")).getAllByTestId("diff");
    expect(diffs.map((d) => [...d.querySelectorAll("[data-kind]")].map((l) => l.textContent))).toEqual([
      ["a", "-b", "+c"],
      ["+x", "+y"],
    ]);
    expect(edit!.textContent).not.toContain("*** Begin Patch");
    fireEvent.click(files);

    expect(write!.textContent).toContain("Write");
    expect(within(write!).getByRole("button").textContent).toBe("b.md");
    expect(row(input!)).toContain("Input|npm run dev");
    expect(row(ask!)).toContain("Ask|Which mode?");
    expect(row(script!)).toContain("Script|const a = 1;");
    fireEvent.click(within(script!).getByRole("button"));
    expect(script!.querySelector("pre")?.textContent).toBe("const a = 1;\nreturn a;\n\ncall_count: 2");
    controller.dispose();
  });

  it("reads Codex's live tool calls the same way", async () => {
    const t = new FakeTransport();
    const { controller, frame } = await openSession(t);
    await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot({ status: "prompting" }) });
    const call = (seq: number, tool_call_id: string, title: string, kind: string, content: string | null = null) =>
      frame({
        type: "event",
        event: { session_id: "s1", seq, type: "agent.tool_call", payload: { tool_call_id, title, kind, status: "in_progress", content } },
      });
    await call(2, "k1", "ls -la && cat notes.txt", "execute", "[Terminal]");
    await call(3, "k2", "Editing files", "edit", "--- hello.txt\n+++ hello.txt\n+hi\n\n--- notes.txt\n+++ notes.txt\n-alpha\n-beta\n+alpha\n+BETA");
    await call(4, "k3", "Read file 'src/a.ts'", "read");
    await call(5, "k4", "Search for 'todo' in src", "search");
    await call(6, "k5", "List files in 'src'", "read");
    await call(7, "k6", "mcp.zwiim.session_info", "execute");
    await call(8, "k7", `"sed -n '1,9p' a.jsx && echo \\"hi\\""`, "execute");
    await frame({
      type: "event",
      event: { session_id: "s1", seq: 9, type: "agent.tool_call_update", payload: { tool_call_id: "k1", title: null, status: "completed", content: "notes.txt\nalpha" } },
    });
    // The Host titles Zwiim's own Git tools in words, whatever the agent called them.
    await call(10, "k8", "Create pull request", "other", "Opened #12 · https://github.com/o/r/pull/12");
    const [run, edit, read, search, list, mcp, quoted, pr] = screen.getAllByTestId("tool-call");
    expect(pr!.textContent).toBe("Create pull requestrunningOpened #12 · https://github.com/o/r/pull/12");
    fireEvent.click(within(run!).getByRole("button"));
    expect([...run!.querySelectorAll("pre")].map((p) => p.textContent)).toEqual(["$ ls -la && cat notes.txt", "notes.txt\nalpha"]);
    expect(edit!.textContent).toContain("Edit");
    expect(edit!.textContent).not.toContain("Editing files");
    expect(within(edit!).getByRole("button").textContent).toBe("2 files");
    expect(within(edit!).getByTestId("diff-counts").textContent).toBe("+2−1");
    expect(read!.textContent).toBe("Reada.tsrunning");
    expect(read!.querySelector("code")?.getAttribute("title")).toBe("src/a.ts");
    expect(search!.textContent).toBe("Search'todo' in srcrunning");
    expect(list!.textContent).toBe("Listsrcrunning");
    expect(mcp!.textContent).toBe("session_infozwiimrunning");
    // `zsh -lc "…"`: the script, not the shell word.
    expect(quoted!.querySelector("code")?.textContent).toBe(`sed -n '1,9p' a.jsx && echo "hi"`);
    controller.dispose();
  });

  it("says why a turn stopped early", async () => {
    const t = new FakeTransport();
    const { controller, frame } = await openSession(t);
    await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot({ status: "prompting" }) });
    await frame({ type: "event", event: { session_id: "s1", seq: 2, type: "agent.turn_complete", payload: { stop_reason: "cancelled" } } });
    expect(screen.getByTestId("stop-reason").textContent).toContain("cancelled");
    controller.dispose();
  });

  it("confirms before cancelling the agent's turn", async () => {
    const t = new FakeTransport();
    t.handlers["session.cancel"] = () => ({});
    const { controller, frame } = await openSession(t);
    await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot({ status: "prompting" }) });
    fireEvent.click(screen.getByLabelText("Stop agent"));
    expect(screen.getByTestId("confirm-cancel")).toBeTruthy();
    fireEvent.click(screen.getByText("Keep going"));
    await tick();
    expect(t.calls.some((c) => c.op === "session.cancel")).toBe(false);
    fireEvent.click(screen.getByLabelText("Stop agent"));
    fireEvent.click(screen.getByText("Stop"));
    await tick();
    expect(t.calls.filter((c) => c.op === "session.cancel")).toHaveLength(1);
    controller.dispose();
  });

  it("tags each send with a client message id", async () => {
    const t = new FakeTransport();
    t.handlers["session.follow_up"] = () => ({ outcome: "sent" });
    const { controller, frame } = await openSession(t);
    await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot() });
    const box = screen.getByPlaceholderText(/^Send a message/);
    fireEvent.change(box, { target: { value: "hello" } });
    fireEvent.keyDown(box, { key: "Enter" });
    await tick();
    const sent = t.calls.find((c) => c.op === "session.follow_up")!.params;
    expect(sent.blocks).toEqual([{ type: "text", text: "hello" }]);
    expect(sent.client_message_id).toMatch(/^[0-9a-f-]{36}$/);
    controller.dispose();
  });

  it("offers a jump to the latest content once scrolled up", async () => {
    const t = new FakeTransport();
    const { controller } = await openSession(t);
    const el = screen.getByTestId("transcript");
    Object.defineProperty(el, "scrollHeight", { configurable: true, value: 1000 });
    Object.defineProperty(el, "clientHeight", { configurable: true, value: 300 });
    el.scrollTop = 100;
    fireEvent.scroll(el);
    // Positioned, so nothing absolutely placed inside (screen-reader labels)
    // escapes it and scrolls the whole window.
    expect(el.className).toContain("relative");
    fireEvent.click(screen.getByText("Jump to latest"));
    // It glides down rather than jumping.
    expect(el.scrollTop).toBe(100);
    await vi.waitFor(() => expect(el.scrollTop).toBe(700), { timeout: 3000 });
    expect(screen.queryByText("Jump to latest")).toBeNull();
    controller.dispose();
  });
});

describe("reset to a message", () => {
  const worktreeW1: WorkspaceInfo = {
    workspace_id: "w1",
    name: "a9b70af5",
    alias: null,
    kind: "worktree",
    parent_workspace_id: "root",
    is_git_repo: true,
    branch: "zwiim/a9b70af5",
    base_branch: "main",
    base_commit: null,
    removed: false,
    display_path: null,
  };
  const said = (id: string, text: string, revertible?: boolean): HistoryTurn => ({
    ...turn(id, "user", [{ type: "text", text }]),
    ...(revertible === undefined ? {} : { revertible }),
  });
  const answered = (id: string, text: string) => turn(id, "assistant", [{ type: "text", text }]);

  async function resetting(
    opts: { canRewind?: boolean; turns?: HistoryTurn[]; revert?: (p: any) => unknown; workspace?: Partial<WorkspaceInfo> } = {},
  ) {
    const t = new FakeTransport();
    if (opts.workspace) t.handlers["workspace.list"] = () => [{ ...worktreeW1, ...opts.workspace }];
    t.handlers["session.history"] = (p) => ({
      session_id: p.session_id,
      turns: opts.turns ?? [said("u1", "first", true), answered("a1", "done"), said("u2", "second", true), answered("a2", "ok")],
    });
    t.handlers["session.options"] = (p) => ({
      session_id: p.session_id,
      options: { model: null, effort: null, mode: null, ...(opts.canRewind === undefined ? {} : { can_rewind_chat: opts.canRewind }) },
    });
    t.handlers["session.revert"] = opts.revert ?? (() => ({ files_restored: true, chat_rewound: true, head_moved: false }));
    const opened = await openSession(t);
    await opened.frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot() });
    return { t, ...opened };
  }
  const reverts = (t: FakeTransport) => t.calls.filter((c) => c.op === "session.revert").map((c) => c.params);

  it("offers a reset only on the messages the Host can put back", async () => {
    const { controller } = await resetting({
      turns: [said("u1", "first", false), answered("a1", "done"), said("u2", "second", true), answered("a2", "ok"), said("u3", "third")],
    });
    expect(screen.getAllByLabelText("Reset to here")).toHaveLength(1);
    fireEvent.click(screen.getByLabelText("Reset to here"));
    expect(screen.getByRole("dialog")).toBeTruthy();
    controller.dispose();
  });

  it("resets the chat and the files to before a message", async () => {
    const { t, controller } = await resetting({ canRewind: true });
    fireEvent.click(screen.getAllByLabelText("Reset to here")[1]!);
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByText("Reset chat to here?")).toBeTruthy();
    expect(within(dialog).getByText("Later messages will be removed.")).toBeTruthy();
    const files = within(dialog).getByLabelText("Also reset files") as HTMLInputElement;
    expect(files.checked).toBe(true);
    // Resetting files discards the user's own uncommitted edits, and the dialog says so.
    expect(within(dialog).getByText("Uncommitted changes made since this message, including your own, will be discarded.")).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Reset" }));
    await tick();
    expect(reverts(t)).toEqual([{ session_id: "s1", turn_id: "u2", files: true, chat: true }]);
    expect(screen.queryByRole("dialog")).toBeNull();
    controller.dispose();
  });

  it("leaves the files alone when unchecked", async () => {
    const { t, controller } = await resetting({ canRewind: true });
    fireEvent.click(screen.getAllByLabelText("Reset to here")[1]!);
    expect(screen.getByText("Uncommitted changes made since this message, including your own, will be discarded.")).toBeTruthy();
    fireEvent.click(screen.getByLabelText("Also reset files"));
    // A chat-only reset keeps the files, so there is nothing to warn about.
    expect(screen.queryByText("Uncommitted changes made since this message, including your own, will be discarded.")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Reset" }));
    await tick();
    expect(reverts(t)).toEqual([{ session_id: "s1", turn_id: "u2", files: false, chat: true }]);
    controller.dispose();
  });

  it("resets only the files when the agent cannot take its chat back", async () => {
    const { t, controller } = await resetting({ canRewind: false });
    fireEvent.click(screen.getAllByLabelText("Reset to here")[1]!);
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByText("Reset files to here?")).toBeTruthy();
    expect(within(dialog).queryByText("Later messages will be removed.")).toBeNull();
    expect(within(dialog).queryByLabelText("Also reset files")).toBeNull();
    expect(within(dialog).getByText("Uncommitted changes made since this message, including your own, will be discarded.")).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Reset files" }));
    await tick();
    expect(reverts(t)).toEqual([{ session_id: "s1", turn_id: "u2", files: true, chat: false }]);
    controller.dispose();
  });

  it("resets only the files to before the first message, which has no reply before it", async () => {
    const { t, controller } = await resetting({ canRewind: true });
    fireEvent.click(screen.getAllByLabelText("Reset to here")[0]!);
    expect(screen.getByText("Reset files to here?")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Reset files" }));
    await tick();
    expect(reverts(t)).toEqual([{ session_id: "s1", turn_id: "u1", files: true, chat: false }]);
    controller.dispose();
  });

  it("resets only the files on a Host that cannot take a chat back", async () => {
    const t = new FakeTransport();
    t.handlers["host.hello"] = () => hostHello({ capabilities: CLIENT_CAPABILITIES.filter((c) => c !== "session.revert.chat") });
    t.handlers["session.history"] = (p) => ({ session_id: p.session_id, turns: [answered("a0", "hi"), said("u1", "go", true)] });
    t.handlers["session.options"] = (p) => ({ session_id: p.session_id, options: { model: null, effort: null, mode: null, can_rewind_chat: true } });
    t.handlers["session.revert"] = () => ({ files_restored: true, chat_rewound: false, head_moved: false });
    const { controller, frame } = await openSession(t);
    await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot() });
    fireEvent.click(screen.getByLabelText("Reset to here"));
    fireEvent.click(screen.getByRole("button", { name: "Reset files" }));
    await tick();
    expect(t.calls.find((c) => c.op === "session.revert")!.params).toMatchObject({ files: true, chat: false });
    controller.dispose();
  });

  it("says commits made since stay, once the reset is done", async () => {
    const { controller } = await resetting({
      canRewind: true,
      revert: () => ({ files_restored: true, chat_rewound: true, head_moved: true }),
    });
    expect(screen.queryByText(/Commits made after this point stay/)).toBeNull();
    fireEvent.click(screen.getAllByLabelText("Reset to here")[1]!);
    fireEvent.click(screen.getByRole("button", { name: "Reset" }));
    await tick();
    expect(
      screen.getByText("Commits made after this point stay; their changes are undone as uncommitted edits."),
    ).toBeTruthy();
    controller.dispose();
  });

  it("says when the files were reset but the chat was not", async () => {
    const { controller } = await resetting({
      canRewind: true,
      revert: () => ({ files_restored: true, chat_rewound: false, head_moved: false }),
    });
    fireEvent.click(screen.getAllByLabelText("Reset to here")[1]!);
    fireEvent.click(screen.getByRole("button", { name: "Reset" }));
    await tick();
    expect(screen.getByText(/files were reset, but the chat could not be taken back/i)).toBeTruthy();
    controller.dispose();
  });

  it("keeps the dialog open and says why the Host refused", async () => {
    let refusal = new ZwiimClientError("checkpoint_unavailable", "no checkpoint", 409);
    const { t, controller } = await resetting({
      canRewind: true,
      revert: () => {
        throw refusal;
      },
    });
    fireEvent.click(screen.getAllByLabelText("Reset to here")[1]!);
    const reset = () => fireEvent.click(screen.getByRole("button", { name: "Reset" }));
    reset();
    await tick();
    expect(within(screen.getByRole("dialog")).getByRole("alert").textContent).toBe("This point can't be restored.");
    refusal = new ZwiimClientError("workspace_busy", "busy", 409, { live_sessions: 0, terminals: 0, session_ids: [], operation: "rebase" });
    reset();
    await tick();
    expect(screen.getByRole("alert").textContent).toMatch(/workspace is busy: a rebase is in progress/i);
    refusal = new ZwiimClientError("workspace_busy", "busy", 409, { live_sessions: 1, terminals: 0, session_ids: ["s2"] });
    reset();
    await tick();
    expect(screen.getByRole("alert").textContent).toMatch(/workspace is busy: another agent is working in it/i);
    refusal = new ZwiimClientError("turn_in_progress", "turn", 409);
    reset();
    await tick();
    expect(screen.getByRole("alert").textContent).toMatch(/agent is still working/i);
    refusal = new ZwiimClientError("root_workspace", "root", 409);
    reset();
    await tick();
    expect(screen.getByRole("alert").textContent).toMatch(/repository folder/i);
    expect(reverts(t)).toHaveLength(5);
    controller.dispose();
  });

  it("offers no reset in a removed worktree or the repository folder", async () => {
    for (const workspace of [{ removed: true, branch: null }, { kind: "root" as const, parent_workspace_id: null, base_branch: null }]) {
      const { controller } = await resetting({ canRewind: true, workspace });
      expect(screen.getByText("second")).toBeTruthy();
      expect(screen.queryByLabelText("Reset to here")).toBeNull();
      controller.dispose();
      cleanup();
    }
    // The same history in a live worktree offers it.
    const { controller } = await resetting({ canRewind: true, workspace: {} });
    expect(screen.getAllByLabelText("Reset to here")).toHaveLength(2);
    controller.dispose();
  });

  it("sends one reset while it is pending, and holds the dialog open until it ends", async () => {
    let finish!: (r: unknown) => void;
    const { t, controller } = await resetting({ canRewind: true, revert: () => new Promise((r) => (finish = r)) });
    fireEvent.click(screen.getAllByLabelText("Reset to here")[1]!);
    const confirm = screen.getByRole("button", { name: "Reset" }) as HTMLButtonElement;
    fireEvent.click(confirm);
    await tick();
    expect(confirm.disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Cancel" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(confirm);
    fireEvent.keyDown(document, { key: "Escape" });
    await tick();
    expect(reverts(t)).toHaveLength(1);
    expect(screen.getByRole("dialog")).toBeTruthy();
    await act(async () => finish({ files_restored: true, chat_rewound: true, head_moved: false }));
    await tick();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(reverts(t)).toHaveLength(1);
    controller.dispose();
  });

  it("holds the reset while the agent works", async () => {
    const { controller, frame } = await resetting({ canRewind: true });
    await frame({ type: "snapshot", epoch: "e", event_seq: 2, snapshot: snapshot({ status: "prompting" }) });
    for (const b of screen.getAllByLabelText("Reset to here")) expect((b as HTMLButtonElement).disabled).toBe(true);
    controller.dispose();
  });
});

describe("tool runs", () => {
  const call = (id: string, title: string, input: string) =>
    ({ type: "tool_call", tool_call_id: id, title, input_preview: input, status: null }) as const;
  const result = (id: string, is_error = false) =>
    ({ type: "tool_result", tool_call_id: id, output_preview: "out", is_error }) as const;

  it("folds three or more tool calls in a row into one row that opens to list them", async () => {
    const t = new FakeTransport();
    t.handlers["session.history"] = (p) => ({
      session_id: p.session_id,
      turns: [
        turn("t1", "assistant", [
          call("c1", "Read", "a.ts"),
          result("c1"),
          call("c2", "Read", "b.ts"),
          result("c2", true),
          { type: "text", text: " " },
          call("c3", "Read", "c.ts"),
          result("c3"),
          { type: "text", text: "Looked around." },
          call("c4", "Read", "d.ts"),
          call("c5", "Read", "e.ts"),
        ]),
      ],
    });
    const { controller } = await openSession(t);
    // A blank text does not split the run; a reply does, and two in a row stay as they are.
    const run = screen.getByTestId("tool-run");
    const row = within(run).getAllByRole("button")[0]!;
    expect(row.textContent).toBe("3 tool calls 1 failed");
    expect(within(run).getAllByTestId("tool-call")).toHaveLength(3);
    expect(screen.getAllByTestId("tool-call")).toHaveLength(5);
    expect(row.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(row);
    expect(row.getAttribute("aria-expanded")).toBe("true");
    controller.dispose();
  });

  it("folds nothing and opens thinking where every tool call shows", async () => {
    rememberExpandToolCalls(true);
    const t = new FakeTransport();
    t.handlers["session.history"] = (p) => ({
      session_id: p.session_id,
      turns: [
        turn("t1", "assistant", [
          { type: "thought", text: "Planning the fix" },
          call("c1", "Read", "a.ts"),
          call("c2", "Read", "b.ts"),
          call("c3", "Read", "c.ts"),
        ]),
      ],
    });
    const { controller } = await openSession(t);
    expect(screen.queryByTestId("tool-run")).toBeNull();
    expect(screen.getAllByTestId("tool-call")).toHaveLength(3);
    const thought = within(screen.getByTestId("thought")).getByRole("button");
    expect(thought.getAttribute("aria-expanded")).toBe("true");
    // Closed by hand, it stays closed; folding comes back when the setting goes off.
    fireEvent.click(thought);
    expect(thought.getAttribute("aria-expanded")).toBe("false");
    act(() => rememberExpandToolCalls(false));
    expect(screen.getByTestId("tool-run")).toBeTruthy();
    controller.dispose();
  });

  it("folds a live run as it grows, its count shimmering while one runs", async () => {
    const t = new FakeTransport();
    const { controller, frame } = await openSession(t);
    await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot({ status: "prompting" }) });
    const start = (seq: number, id: string) =>
      frame({
        type: "event",
        event: { session_id: "s1", seq, type: "agent.tool_call", payload: { tool_call_id: id, title: `Read ${id}.ts`, kind: "read", status: "in_progress", content: null } },
      });
    await start(2, "a");
    await start(3, "b");
    expect(screen.queryByTestId("tool-run")).toBeNull();
    await start(4, "c");
    const run = screen.getByTestId("tool-run");
    expect(run.textContent).toContain("3 tool calls");
    expect(within(run).getByText("3 tool calls").className).toContain("animate-shimmer");
    for (const [seq, id] of [[5, "a"], [6, "b"], [7, "c"]] as const)
      await frame({
        type: "event",
        event: { session_id: "s1", seq, type: "agent.tool_call_update", payload: { tool_call_id: id, title: null, status: "completed", content: null } },
      });
    expect(within(screen.getByTestId("tool-run")).getByText("3 tool calls").className).not.toContain("animate-shimmer");
    controller.dispose();
  });

  it("folds sub-agents in with the tool calls, a failed one counted with the failed calls", async () => {
    const t = new FakeTransport();
    t.handlers["session.history"] = (p) => ({
      session_id: p.session_id,
      turns: [
        turn("t1", "assistant", [
          call("c1", "Read", "a.ts"),
          result("c1", true),
          {
            type: "tool_call",
            tool_call_id: "a1",
            title: "Task",
            input_preview: null,
            status: "failed",
            subagent: { description: "Dig", blocks: [{ type: "text", text: "dug in" }] },
          },
          call("c2", "Read", "b.ts"),
          { type: "text", text: "Done." },
        ]),
      ],
    });
    const { controller } = await openSession(t);
    const run = screen.getByTestId("tool-run");
    const row = within(run).getAllByRole("button")[0]!;
    expect(row.textContent).toBe("2 tool calls, 1 sub-agent 2 failed");
    fireEvent.click(row);
    // The card inside the fold still opens to what its sub-agent did.
    const card = within(run).getByTestId("subagent-card");
    fireEvent.click(card.querySelector<HTMLButtonElement>(":scope > button")!);
    expect(within(card).getByTestId("subagent-body").textContent).toContain("dug in");
    controller.dispose();
  });

  it("keeps a live fold shimmering while a sub-agent in it runs, its clock ticking", async () => {
    const t = new FakeTransport();
    const { controller, frame } = await openSession(t);
    await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot({ status: "prompting" }) });
    const ev = (seq: number, type: string, payload: object) =>
      frame({ type: "event", event: { session_id: "s1", seq, type, payload } });
    const tool = (id: string) => ({
      tool_call_id: id,
      title: `Read ${id}.ts`,
      kind: "read",
      status: "in_progress",
      content: null,
    });
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "Date"] });
    try {
      vi.setSystemTime(new Date("2026-10-24T10:00:30Z"));
      await ev(2, "agent.tool_call", tool("a"));
      await ev(3, "agent.tool_call", tool("b"));
      await ev(4, "agent.tool_call", {
        ...tool("fold1"),
        title: "Agent",
        kind: "other",
        subagent: { description: "Survey", started_at: "2026-10-24T10:00:00.000Z" },
      });
      for (const [seq, id] of [
        [5, "a"],
        [6, "b"],
      ] as const)
        await ev(seq, "agent.tool_call_update", { tool_call_id: id, title: null, status: "completed", content: null });
      const run = screen.getByTestId("tool-run");
      const label = within(run).getByText("2 tool calls, 1 sub-agent");
      expect(label.className).toContain("animate-shimmer");
      const card = within(run).getByTestId("subagent-card");
      expect(within(card).getByTestId("subagent-elapsed").textContent).toBe("30s");
      act(() => void vi.advanceTimersByTime(2000));
      expect(within(card).getByTestId("subagent-elapsed").textContent).toBe("32s");
      await ev(7, "agent.tool_call_update", { tool_call_id: "fold1", title: null, status: "completed", content: null });
      expect(within(run).getByText("2 tool calls, 1 sub-agent").className).not.toContain("animate-shimmer");
    } finally {
      vi.useRealTimers();
    }
    controller.dispose();
  });

  it("carries the running turn's run from the history on into the live turn, as one fold", async () => {
    const t = new FakeTransport();
    const user = turn("u1", "user", [{ type: "text", text: "go" }]);
    const before = [call("c1", "Read", "a.ts"), result("c1"), call("c2", "Read", "b.ts"), result("c2")];
    t.handlers["session.history"] = (p) => ({
      session_id: p.session_id,
      turns: [user, turn("a1", "assistant", before)],
    });
    const { controller, frame } = await openSession(t);
    // Reopened mid-turn: the history holds the turn the agent is still running.
    await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot({ status: "prompting" }) });
    expect(screen.queryByTestId("tool-run")).toBeNull();
    const start = (seq: number, id: string) =>
      frame({
        type: "event",
        event: {
          session_id: "s1",
          seq,
          type: "agent.tool_call",
          payload: { tool_call_id: id, title: `Read ${id}.ts`, kind: "read", status: "in_progress", content: null },
        },
      });
    await start(2, "c3");
    const run = screen.getByTestId("tool-run");
    expect(within(screen.getByTestId("history")).getByTestId("tool-run")).toBe(run);
    expect(within(run).getByText("3 tool calls").className).toContain("animate-shimmer");
    expect(within(run).getAllByTestId("tool-call")).toHaveLength(3);
    expect(within(screen.getByTestId("live-turn")).queryAllByTestId("tool-call")).toHaveLength(0);
    const row = within(run).getAllByRole("button")[0]!;
    fireEvent.click(row);

    // It grows in place: the same row, still open.
    await start(3, "c4");
    expect(screen.getByTestId("tool-run")).toBe(run);
    expect(row.getAttribute("aria-expanded")).toBe("true");
    expect(row.textContent).toBe("4 tool calls");

    // The turn ends and the history catches up: the fold is where it was, still open.
    const after = [...before, call("c3", "Read", "c3.ts"), result("c3"), call("c4", "Read", "c4.ts"), result("c4")];
    t.handlers["session.history"] = (p) => ({
      session_id: p.session_id,
      turns: [user, turn("a1", "assistant", after)],
    });
    await frame({
      type: "event",
      event: { session_id: "s1", seq: 4, type: "agent.turn_complete", payload: { stop_reason: "end_turn" } },
    });
    await act(() => tick());
    const runs = screen.getAllByTestId("tool-run");
    expect(runs).toHaveLength(1);
    expect(within(runs[0]!).getAllByRole("button")[0]!.getAttribute("aria-expanded")).toBe("true");
    expect(within(runs[0]!).getAllByTestId("tool-call")).toHaveLength(4);
    controller.dispose();
  });

  it("does not carry a finished turn's run into the next one", async () => {
    const t = new FakeTransport();
    t.handlers["session.history"] = (p) => ({
      session_id: p.session_id,
      turns: [
        turn("u1", "user", [{ type: "text", text: "go" }]),
        turn("a1", "assistant", [call("c1", "Read", "a.ts"), call("c2", "Read", "b.ts")]),
      ],
    });
    const { controller, frame } = await openSession(t);
    await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot({ status: "connected" }) });
    await frame({
      type: "event",
      event: {
        session_id: "s1",
        seq: 2,
        type: "agent.user_message",
        payload: { message_id: "m2", blocks: [{ type: "text", text: "more" }] },
      },
    });
    await frame({
      type: "event",
      event: {
        session_id: "s1",
        seq: 3,
        type: "agent.tool_call",
        payload: { tool_call_id: "c3", title: "Read c.ts", kind: "read", status: "in_progress", content: null },
      },
    });
    expect(screen.queryByTestId("tool-run")).toBeNull();
    expect(within(screen.getByTestId("history")).getAllByTestId("tool-call")).toHaveLength(2);
    expect(within(screen.getByTestId("live-turn")).getAllByTestId("tool-call")).toHaveLength(1);
    controller.dispose();
  });

  /** The history holds the turn the agent is still running, which ends with a call; the live reply goes on with more. */
  async function runningTurn(t: FakeTransport, before: HistoryTurn["blocks"]) {
    const user = turn("u1", "user", [{ type: "text", text: "go" }]);
    t.handlers["session.history"] = (p) => ({
      session_id: p.session_id,
      turns: [user, turn("a1", "assistant", before)],
    });
    const opened = await openSession(t);
    await opened.frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot({ status: "prompting" }) });
    const start = (seq: number, id: string) =>
      opened.frame({
        type: "event",
        event: {
          session_id: "s1",
          seq,
          type: "agent.tool_call",
          payload: { tool_call_id: id, title: `Read ${id}.ts`, kind: "read", status: "completed", content: "out" },
        },
      });
    const history = (after: HistoryTurn["blocks"]) => (p: { session_id: string }) => ({
      session_id: p.session_id,
      turns: [user, turn("a1", "assistant", after)],
    });
    const finish = (seq: number) =>
      opened.frame({
        type: "event",
        event: { session_id: "s1", seq, type: "agent.turn_complete", payload: { stop_reason: "end_turn" } },
      });
    return { ...opened, start, history, finish };
  }
  const expanded = (el: HTMLElement) => within(el).getAllByRole("button")[0]!.getAttribute("aria-expanded");

  it("keeps a call opened live open as it folds and once the history has it", async () => {
    const t = new FakeTransport();
    const before = [call("c1", "Read", "a.ts"), result("c1")];
    const { controller, start, history, finish } = await runningTurn(t, before);
    await start(2, "c2");
    fireEvent.click(within(screen.getAllByTestId("tool-call")[1]!).getAllByRole("button")[0]!);

    // A third call folds the run: the call is still open inside it.
    await start(3, "c3");
    const run = screen.getByTestId("tool-run");
    fireEvent.click(within(run).getAllByRole("button")[0]!);
    expect(within(run).getAllByTestId("tool-call").map(expanded)).toEqual(["false", "true", "false"]);

    t.handlers["session.history"] = history([
      ...before,
      call("c2", "Read", "c2.ts"),
      result("c2"),
      call("c3", "Read", "c3.ts"),
      result("c3"),
    ]);
    await finish(4);
    await act(() => tick());
    const calls = within(screen.getByTestId("tool-run")).getAllByTestId("tool-call");
    expect(calls.map(expanded)).toEqual(["false", "true", "false"]);
    controller.dispose();
  });

  it("keeps the run carried on into a finished turn as one fold until the history has the turn", async () => {
    const t = new FakeTransport();
    const before = [call("c1", "Read", "a.ts"), result("c1"), call("c2", "Read", "b.ts"), result("c2")];
    const { controller, start, history, finish } = await runningTurn(t, before);
    await start(2, "c3");
    const run = screen.getByTestId("tool-run");
    const row = within(run).getAllByRole("button")[0]!;
    fireEvent.click(row);

    // The turn ends; the history is slow to come back.
    let reload: () => void = () => {};
    const slow = history([...before, call("c3", "Read", "c3.ts"), result("c3")]);
    t.handlers["session.history"] = (p) => new Promise((done) => (reload = () => done(slow(p))));
    await finish(3);
    await act(() => tick());
    expect(screen.getAllByTestId("tool-run")).toEqual([run]);
    expect(row.getAttribute("aria-expanded")).toBe("true");
    expect(within(run).getAllByTestId("tool-call")).toHaveLength(3);
    for (const live of screen.queryAllByTestId("live-turn"))
      expect(within(live).queryAllByTestId("tool-call")).toHaveLength(0);

    await act(async () => reload());
    await act(() => tick());
    const runs = screen.getAllByTestId("tool-run");
    expect(runs).toHaveLength(1);
    expect(expanded(runs[0]!)).toBe("true");
    expect(within(runs[0]!).getAllByTestId("tool-call")).toHaveLength(3);
    controller.dispose();
  });
});

describe("changed files", () => {
  const workspace = {
    workspace_id: "w1",
    name: "a9b70af5",
    alias: null,
    kind: "worktree",
    parent_workspace_id: "root",
    is_git_repo: true,
    branch: "zwiim/a9b70af5",
    base_branch: "main",
    base_commit: null,
    removed: false,
    display_path: null,
  } as const;
  const links = () => ({ workspace, open: vi.fn(), openDiff: vi.fn() });
  const edit = (id: string, path: string, old = "a", next = "b"): HistoryTurn["blocks"] => [
    { type: "tool_call", tool_call_id: id, title: "Edit", input_preview: JSON.stringify({ file_path: path, old_string: old, new_string: next }), status: null },
  ];
  const at = (t: HistoryTurn, timestamp: string, completed_at: string | null = null): HistoryTurn => ({ ...t, timestamp, completed_at });
  const history = (turns: HistoryTurn[]) => (p: { session_id: string }) => ({ session_id: p.session_id, turns });

  it("opens an edited file whole from its chip; a file outside the workspace stays inert", async () => {
    const t = new FakeTransport();
    t.handlers["session.history"] = history([
      turn("t1", "assistant", [
        ...edit("c1", "/Users/me/zwiim/worktrees/zwiim/a9b70af5/src/a.ts"),
        ...edit("c2", "/etc/hosts"),
        { type: "tool_call", tool_call_id: "c3", title: "apply_patch", input_preview: "*** Begin Patch\n*** Update File: src/b.ts\n@@\n-x\n+y\n*** Update File: src/c.ts\n@@\n-x\n+y\n*** End Patch", status: null },
      ]),
    ]);
    const l = links();
    const { controller } = await openSession(t, l);
    const [inside, outside, several] = screen.getAllByTestId("tool-call");
    fireEvent.click(within(inside!).getByRole("button", { name: "a.ts" }));
    expect(l.openDiff).toHaveBeenCalledWith("src/a.ts");
    expect(screen.queryByTestId("hover-card")).toBeNull();

    // Outside the workspace the chip only shows the change.
    fireEvent.click(within(outside!).getByRole("button", { name: "hosts" }));
    expect(l.openDiff).toHaveBeenCalledTimes(1);
    expect(screen.getByTestId("hover-card")).toBeTruthy();
    fireEvent.click(within(outside!).getByRole("button", { name: "hosts" }));

    // Several files: the chip shows them all, and each one opens.
    fireEvent.click(within(several!).getByRole("button", { name: /2 files/ }));
    fireEvent.click(within(screen.getByTestId("hover-card")).getByRole("button", { name: /src\/c\.ts/ }));
    expect(l.openDiff).toHaveBeenLastCalledWith("src/c.ts");
    controller.dispose();
  });

  it("ends each turn with its time and files, two at a time", async () => {
    const t = new FakeTransport();
    const files = ["a", "b", "c", "d", "e"].map((n, i) => edit(`c${i}`, `src/${n}.ts`)).flat();
    t.handlers["session.history"] = history([
      at(turn("u1", "user", [{ type: "text", text: "go" }]), "2026-10-06T06:32:00Z"),
      at(turn("a1", "assistant", files), "2026-10-06T06:33:00Z", "2026-10-06T06:41:31Z"),
      at(turn("u2", "user", [{ type: "text", text: "thanks" }]), "2026-10-06T06:42:00Z"),
      at(turn("a2", "assistant", [{ type: "text", text: "ok" }]), "2026-10-06T06:42:05Z", "2026-10-06T06:42:12Z"),
    ]);
    const l = links();
    const { controller } = await openSession(t, l);
    const [first, second] = screen.getAllByTestId("turn-footer");
    expect(within(first!).getByTestId("turn-time").textContent).toBe(`9m 31s · ${clockTime("2026-10-06T06:41:31Z")}`);
    const names = () => within(first!).queryAllByTestId("turn-file").map((f) => f.textContent);
    expect(names()).toEqual(["a.ts+1−1", "b.ts+1−1"]);
    const more = within(first!).getByRole("button", { name: /\+3 more/ });
    expect(more.textContent).toBe("+3 more+3−3");
    fireEvent.click(more);
    expect(names()).toHaveLength(4);
    fireEvent.click(within(first!).getByRole("button", { name: /\+1 more/ }));
    expect(names()).toHaveLength(5);
    fireEvent.click(within(first!).getByRole("button", { name: "Show less" }));
    expect(names()).toHaveLength(2);
    fireEvent.click(within(first!).getAllByTestId("turn-file")[1]!);
    expect(l.openDiff).toHaveBeenCalledWith("src/b.ts");

    // A turn that changed nothing shows only its time.
    expect(within(second!).getByTestId("turn-time").textContent).toBe(`12s · ${clockTime("2026-10-06T06:42:12Z")}`);
    expect(within(second!).queryAllByTestId("turn-file")).toHaveLength(0);
    expect(within(second!).queryByRole("button")).toBeNull();
    controller.dispose();
  });

  describe("forking a turn", () => {
    const forkable = (targets: ForkTarget[] = ["current", "new_worktree"]) =>
      history([
        at(turn("u1", "user", [{ type: "text", text: "go" }]), "2026-10-06T06:32:00Z"),
        { ...at(turn("a1", "assistant", [{ type: "text", text: "ok" }]), "", ""), fork_targets: targets },
        at(turn("u2", "user", [{ type: "text", text: "more" }]), "2026-10-06T06:42:00Z"),
        { ...at(turn("a2", "assistant", [{ type: "text", text: "done" }]), "", ""), fork_targets: targets },
      ]);

    /** Opens the turn's fork icon, then finds one of its options. */
    const forkOption = (footer: HTMLElement, name: string) => {
      const icon = within(footer).getByRole("button", { name: "Fork" });
      if (icon.getAttribute("aria-expanded") !== "true") fireEvent.click(icon);
      return within(footer).getByRole("button", { name }) as HTMLButtonElement;
    };

    it("offers what the Host offers, even with no time or files to show", async () => {
      const t = new FakeTransport();
      t.handlers["session.history"] = forkable(["new_worktree"]);
      const { controller } = await openSession(t);
      const [first] = screen.getAllByTestId("turn-footer");
      expect(forkOption(first!, "Fork to new worktree")).not.toBeNull();
      expect(within(first!).queryByRole("button", { name: "Fork here" })).toBeNull();
      controller.dispose();
    });

    it("puts the fork icon right after the time, ahead of the files", async () => {
      const t = new FakeTransport();
      t.handlers["session.history"] = history([
        at(turn("u1", "user", [{ type: "text", text: "go" }]), "2026-10-06T06:32:00Z"),
        {
          ...at(turn("a1", "assistant", edit("c1", "/Users/me/zwiim/worktrees/zwiim/a9b70af5/src/a.ts")), "", "2026-10-06T06:32:12Z"),
          fork_targets: ["current", "new_worktree"],
        },
      ]);
      const { controller } = await openSession(t);
      const [first] = screen.getAllByTestId("turn-footer");
      const icon = within(first!).getByRole("button", { name: "Fork" });
      expect(within(first!).getByTestId("turn-time").nextElementSibling).toBe(icon.parentElement);
      expect(icon.compareDocumentPosition(within(first!).getByTestId("turn-file")) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
      controller.dispose();
    });

    it("keeps the options behind the fork icon until it is hovered, and away again after", async () => {
      vi.useFakeTimers({ shouldAdvanceTime: true });
      const t = new FakeTransport();
      t.handlers["session.history"] = forkable();
      const { controller } = await openSession(t);
      const [first] = screen.getAllByTestId("turn-footer");
      const icon = within(first!).getByRole("button", { name: "Fork" });
      expect(within(first!).queryByRole("button", { name: "Fork here" })).toBeNull();
      expect(first!.textContent).not.toContain("Fork here");
      fireEvent.mouseEnter(icon.parentElement!);
      expect(within(first!).getByRole("button", { name: "Fork here" })).not.toBeNull();
      expect(within(first!).getByRole("button", { name: "Fork to new worktree" })).not.toBeNull();
      fireEvent.mouseLeave(icon.parentElement!);
      await act(async () => {
        vi.advanceTimersByTime(200);
      });
      expect(within(first!).queryByRole("button", { name: "Fork here" })).toBeNull();
      vi.useRealTimers();
      controller.dispose();
    });

    it("pins the options on a click, until Escape or a pick", async () => {
      const t = new FakeTransport();
      t.handlers["session.history"] = forkable();
      t.handlers["session.fork"] = () => ({ session_id: "s2", workspace_id: "w2" });
      const { controller } = await openSession(t);
      const [first] = screen.getAllByTestId("turn-footer");
      const icon = within(first!).getByRole("button", { name: "Fork" });
      fireEvent.click(icon);
      expect(icon.getAttribute("aria-expanded")).toBe("true");
      fireEvent.keyDown(icon, { key: "Escape" });
      expect(icon.getAttribute("aria-expanded")).toBe("false");
      fireEvent.click(icon);
      await act(async () => {
        fireEvent.click(within(first!).getByRole("button", { name: "Fork here" }));
      });
      expect(icon.getAttribute("aria-expanded")).toBe("false");
      controller.dispose();
    });

    it("closes on Escape wherever the focus is", async () => {
      const t = new FakeTransport();
      t.handlers["session.history"] = forkable();
      const { controller } = await openSession(t);
      const [first] = screen.getAllByTestId("turn-footer");
      const icon = within(first!).getByRole("button", { name: "Fork" });
      fireEvent.click(icon);
      fireEvent.keyDown(document.body, { key: "Escape" });
      expect(icon.getAttribute("aria-expanded")).toBe("false");
      controller.dispose();
    });

    it("hides the actions from a Host that doesn't fork", async () => {
      const t = new FakeTransport();
      t.handlers["host.hello"] = () => hostHello({ capabilities: CLIENT_CAPABILITIES.filter((c) => c !== "session.fork") });
      t.handlers["session.history"] = forkable();
      const { controller } = await openSession(t);
      expect(screen.queryByRole("button", { name: "Fork" })).toBeNull();
      controller.dispose();
    });

    it("opens the new session where the Host put it", async () => {
      const t = new FakeTransport();
      t.handlers["session.history"] = forkable();
      t.handlers["session.fork"] = () => ({ session_id: "s2", workspace_id: "w2" });
      const open = vi.fn();
      const { controller } = await openSession(t, undefined, open);
      const [first] = screen.getAllByTestId("turn-footer");
      const option = forkOption(first!, "Fork to new worktree");
      await act(async () => {
        fireEvent.click(option);
      });
      expect(t.calls.at(-1)?.params).toEqual({ session_id: "s1", turn_id: "a1", target: "new_worktree" });
      expect(open).toHaveBeenCalledWith("w2", "s2");
      controller.dispose();
    });

    it("hands what's typed to the session it opens", async () => {
      const t = new FakeTransport();
      t.handlers["session.history"] = forkable();
      t.handlers["session.fork"] = () => ({ session_id: "s9", workspace_id: "w2" });
      const { controller } = await openSession(t, undefined, vi.fn());
      fireEvent.change(screen.getByRole("textbox"), { target: { value: "half a thought" } });
      fireEvent.change(screen.getByTestId("attachment-input"), { target: { files: [new File(["x"], "notes.txt")] } });
      const [first] = screen.getAllByTestId("turn-footer");
      const option = forkOption(first!, "Fork here");
      await act(async () => {
        fireEvent.click(option);
      });
      const kept = forkedDraft("s9");
      expect(kept.text).toBe("half a thought");
      expect(kept.files.map((f) => f.name)).toEqual(["notes.txt"]);
      dropForkedDraft("s9");
      controller.dispose();
    });

    it("says why the Host refused, under the turn", async () => {
      const t = new FakeTransport();
      t.handlers["session.history"] = forkable();
      t.handlers["session.fork"] = () => {
        throw new ZwiimClientError("fork_unavailable", "x", 409);
      };
      const open = vi.fn();
      const { controller } = await openSession(t, undefined, open);
      const [first, second] = screen.getAllByTestId("turn-footer");
      const option = forkOption(first!, "Fork here");
      await act(async () => {
        fireEvent.click(option);
      });
      expect(within(first!).getByRole("alert").textContent).toBe("This turn can't be forked.");
      expect(within(second!).queryByRole("alert")).toBeNull();
      expect(open).not.toHaveBeenCalled();
      controller.dispose();
    });

    it("keeps the actions visible but off while the agent runs, and says why", async () => {
      const t = new FakeTransport();
      t.handlers["session.history"] = forkable();
      const { controller, frame } = await openSession(t);
      await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot({ status: "prompting" }) });
      const [first] = screen.getAllByTestId("turn-footer");
      const here = forkOption(first!, "Fork here");
      expect(here.disabled).toBe(true);
      expect(here.title).toBe("Wait for the agent to finish.");
      controller.dispose();
    });

    it("warns only on an older turn that the files stay as they are", async () => {
      const t = new FakeTransport();
      t.handlers["session.history"] = forkable();
      const { controller } = await openSession(t);
      const [first, second] = screen.getAllByTestId("turn-footer");
      expect(forkOption(first!, "Fork here").title).toBe(FORK_OLDER_TURN_NOTE);
      expect(forkOption(second!, "Fork here").title).toBe("");
      controller.dispose();
    });

    it("links to the session a fork came from, in its own workspace", async () => {
      const t = new FakeTransport();
      t.handlers["session.history"] = (p) => ({ session_id: p.session_id, turns: [], forked_from: { session_id: "s0", title: "Old work" } });
      t.handlers["session.list"] = () => [{ session_id: "s0", title: "Old work" }];
      const open = vi.fn();
      const client = await openSession(t, undefined, open);
      await act(async () => {
        await client.controller.refreshSessions("w1");
      });
      fireEvent.click(screen.getByRole("button", { name: "Forked from Old work" }));
      expect(open).toHaveBeenCalledWith("w1", "s0");
      client.controller.dispose();
    });

    it("names a source that isn't listed in plain text", async () => {
      const t = new FakeTransport();
      t.handlers["session.history"] = (p) => ({ session_id: p.session_id, turns: [], forked_from: { session_id: "s0", title: null } });
      const { controller } = await openSession(t);
      expect(screen.getByTestId("forked-from").textContent).toBe("Forked from a session that is gone");
      expect(screen.queryByRole("button", { name: /Forked from/ })).toBeNull();
      controller.dispose();
    });
  });

  it("has no row yet for the turn the agent is still answering", async () => {
    const t = new FakeTransport();
    t.handlers["session.history"] = history([
      at(turn("u1", "user", [{ type: "text", text: "go" }]), "2026-10-06T06:32:00Z"),
      at(turn("a1", "assistant", edit("c1", "src/a.ts")), "2026-10-06T06:33:00Z", "2026-10-06T06:34:00Z"),
    ]);
    const { controller, frame } = await openSession(t, links());
    expect(screen.getAllByTestId("turn-footer")).toHaveLength(1);
    await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot({ status: "prompting" }) });
    expect(screen.queryByTestId("turn-footer")).toBeNull();
    controller.dispose();
  });

  it("shows a read image in the card's row: its file chip is the image and opens it", async () => {
    const t = new FakeTransport();
    t.handlers["files.download"] = (p) => ({ name: p.path.split("/").pop(), mime_type: "image/png", size: 1, offset: 0, data: "AA==" });
    t.handlers["session.history"] = history([
      turn("t1", "assistant", [
        { type: "tool_call", tool_call_id: "r1", title: "Read", input_preview: JSON.stringify({ file_path: "/tmp/shot.png" }), status: "completed" },
        { type: "tool_result", tool_call_id: "r1", output_preview: null, is_error: false, images: [{ name: "ab.png", mime_type: "image/png", path: ".zwiim/images/ab.png" }] },
        { type: "agent_image", name: "cd.png", mime_type: "image/png", path: ".zwiim/images/cd.png", revised_prompt: "a cat" },
      ]),
    ]);
    const l = links();
    const { controller } = await openSession(t, l);
    const card = screen.getByTestId("tool-call");
    expect(card.textContent).toContain("Read image");
    // One chip in the row, named for the file the agent read; nothing under it.
    expect(within(card).getAllByTestId("agent-image")).toHaveLength(1);
    expect(card.querySelector("button button")).toBeNull();
    // The row's chip shows the image icon, not a thumbnail, and fetches nothing.
    expect(card.querySelector("img:not([aria-hidden])")).toBeNull();
    expect(t.calls.some((c) => c.op === "files.download" && c.params.path === ".zwiim/images/ab.png")).toBe(false);
    // The message's image is a chip too.
    expect(screen.getAllByTestId("agent-image")).toHaveLength(2);
    expect(screen.getAllByTestId("agent-image")[1]!.textContent).toContain("cd.png");
    fireEvent.click(within(card).getByRole("button", { name: "Open shot.png" }));
    expect(l.open).toHaveBeenCalledWith(".zwiim/images/ab.png");
    controller.dispose();
  });

  it("shows a live tool call's images", async () => {
    const t = new FakeTransport();
    t.handlers["files.download"] = (p) => ({ name: p.path.split("/").pop(), mime_type: "image/png", size: 1, offset: 0, data: "AA==" });
    const l = links();
    const { controller, frame } = await openSession(t, l);
    await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot({ status: "prompting" }) });
    await frame({ type: "event", event: { session_id: "s1", seq: 2, type: "agent.tool_call", payload: { tool_call_id: "g1", title: "Generate image", kind: "other", status: "completed", content: null, images: [{ name: "ef.png", mime_type: "image/png", path: ".zwiim/images/ef.png" }] } } });
    expect(screen.getByTestId("agent-image").textContent).toContain("ef.png");
    controller.dispose();
  });

  it("opens a written pdf or html in the preview and code in the diff", async () => {
    const t = new FakeTransport();
    t.handlers["session.history"] = history([
      at(turn("u1", "user", [{ type: "text", text: "go" }]), "2026-10-06T06:32:00Z"),
      at(
        turn("t1", "assistant", [
          ...edit("c1", "out/report.pdf"),
          ...edit("c2", "out/page.html"),
          ...edit("c3", "src/a.ts"),
        ]),
        "2026-10-06T06:33:00Z",
        "2026-10-06T06:41:31Z",
      ),
    ]);
    const l = links();
    const { controller } = await openSession(t, l);
    const [pdf, html, ts] = screen.getAllByTestId("tool-call");
    fireEvent.click(within(pdf!).getByRole("button", { name: "report.pdf" }));
    fireEvent.click(within(html!).getByRole("button", { name: "page.html" }));
    fireEvent.click(within(ts!).getByRole("button", { name: "a.ts" }));
    expect(l.open.mock.calls).toEqual([["out/report.pdf"], ["out/page.html"]]);
    expect(l.openDiff.mock.calls).toEqual([["src/a.ts"]]);
    // The turn footer chips split the same way.
    const files = screen.getAllByTestId("turn-file");
    fireEvent.click(files[0]!);
    expect(l.open).toHaveBeenLastCalledWith("out/report.pdf");
    controller.dispose();
  });

  it("shows a live message image inline", async () => {
    const t = new FakeTransport();
    t.handlers["files.download"] = (p) => ({ name: p.path.split("/").pop(), mime_type: "image/png", size: 1, offset: 0, data: "AA==" });
    const { controller, frame } = await openSession(t, links());
    await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot({ status: "prompting" }) });
    await frame({ type: "event", event: { session_id: "s1", seq: 2, type: "agent.message_image", payload: { name: "ab.png", mime_type: "image/png", path: ".zwiim/images/ab.png" } } });
    expect(screen.getByTestId("agent-image").textContent).toContain("ab.png");
    controller.dispose();
  });
});

describe("sub-agent cards", () => {
  const event = (seq: number, type: string, payload: object) => ({ type: "event", event: { session_id: "s1", seq, type, payload } });
  const launch = (tool_call_id: string, subagent: object, extra: object = {}) => ({
    tool_call_id,
    title: "Agent",
    kind: "other",
    status: "in_progress",
    content: null,
    subagent,
    ...extra,
  });
  const cards = () => screen.queryAllByTestId("subagent-card");
  const row = (card: HTMLElement) => card.querySelector<HTMLButtonElement>(":scope > button")!;

  /** A turn with two sub-agents; the first thinks, speaks and runs a tool. */
  async function twoSubagents() {
    const t = new FakeTransport();
    const opened = await openSession(t);
    const { frame } = opened;
    await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot({ status: "prompting" }) });
    await frame(event(2, "agent.message_delta", { text: "Delegating." }));
    await frame(event(3, "agent.tool_call", launch("a1", { description: "Map client add-workspace code", model: "claude-sonnet-5-5", prompt: "Find where the client adds a workspace" })));
    await frame(event(4, "agent.subagent_thought_delta", { parent_id: "a1", text: "child plans" }));
    await frame(event(5, "agent.subagent_message_delta", { parent_id: "a1", text: "child found it" }));
    await frame(event(6, "agent.tool_call", { tool_call_id: "r1", title: "Read src/a.ts", kind: "read", status: "completed", content: null, parent_id: "a1" }));
    await frame(event(7, "agent.tool_call", launch("a2", { description: "Review the diff" })));
    return opened;
  }

  it("is one collapsed row per sub-agent, in launch order, with nothing it did mounted", async () => {
    const { controller } = await twoSubagents();
    const [first, second] = cards();
    expect(cards()).toHaveLength(2);
    expect(within(first!).getByTestId("subagent-name").textContent).toBe("Agent • Sonnet 5.5");
    expect(within(first!).getByTestId("subagent-description").textContent).toBe("Map client add-workspace code");
    expect(within(first!).getByTestId("subagent-tools").textContent).toBe("1 tool");
    expect(within(second!).getByTestId("subagent-description").textContent).toBe("Review the diff");
    // Collapsed: a button that says so, and the sub-agent's content is not in the page.
    expect(row(first!).getAttribute("aria-expanded")).toBe("false");
    expect(within(first!).queryByTestId("subagent-body")).toBeNull();
    expect(screen.queryByText("child found it")).toBeNull();
    expect(screen.queryByTestId("thought")).toBeNull();
    // The launch is the card, not a tool row too; the reply's own text stays.
    expect(screen.queryAllByTestId("tool-call")).toHaveLength(0);
    expect(screen.getByText("Delegating.")).toBeTruthy();
    controller.dispose();
  });

  it("opens to its prompt behind a toggle, then its thinking, text and tool calls", async () => {
    const { controller } = await twoSubagents();
    const [first] = cards();
    fireEvent.click(row(first!));
    expect(row(first!).getAttribute("aria-expanded")).toBe("true");
    const body = within(first!).getByTestId("subagent-body");
    // The prompt stays folded until asked for.
    const toggle = within(body).getByRole("button", { name: "Prompt" });
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(within(body).queryByTestId("subagent-prompt")).toBeNull();
    fireEvent.click(toggle);
    expect(within(body).getByTestId("subagent-prompt").textContent).toBe("Find where the client adds a workspace");
    // Drawn as the transcript draws a reply.
    expect(within(body).getByTestId("thought").textContent).toContain("child plans");
    expect(within(body).getAllByTestId("markdown").map((m) => m.textContent)).toContain("child found it");
    expect(within(body).getByTestId("tool-call").textContent).toBe("Reada.tsdone");
    // Shutting it unmounts what it did again.
    fireEvent.click(row(first!));
    expect(within(first!).queryByTestId("subagent-body")).toBeNull();
    controller.dispose();
  });

  it("omits the model when the agent names none", async () => {
    const { controller } = await twoSubagents();
    expect(within(cards()[1]!).getByTestId("subagent-name").textContent).toBe("Agent");
    controller.dispose();
  });

  it("reads running, then done with its duration, or failed", async () => {
    const { controller, frame } = await twoSubagents();
    const [first, second] = cards();
    expect(first!.getAttribute("data-status")).toBe("in_progress");
    expect(first!.textContent).toContain("running");
    expect(within(first!).queryByTestId("subagent-duration")).toBeNull();
    await frame(event(8, "agent.tool_call_update", { tool_call_id: "a1", title: null, status: "completed", content: null, subagent: { duration_ms: 106_400 } }));
    await frame(event(9, "agent.tool_call_update", { tool_call_id: "a2", title: null, status: "failed", content: null }));
    expect(first!.getAttribute("data-status")).toBe("completed");
    expect(within(first!).getByTestId("subagent-duration").textContent).toBe("1m 46s");
    expect(first!.textContent).toContain("done");
    expect(second!.getAttribute("data-status")).toBe("failed");
    expect(second!.textContent).toContain("failed");
    controller.dispose();
  });

  it("ticks a running sub-agent's time from when it started, then shows its duration once done", async () => {
    const t = new FakeTransport();
    const { controller, frame } = await openSession(t);
    await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot({ status: "prompting" }) });
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "Date"] });
    try {
      vi.setSystemTime(new Date("2026-10-24T10:01:05Z"));
      await frame(
        event(
          2,
          "agent.tool_call",
          launch("clock1", { description: "Survey", started_at: "2026-10-24T10:00:00.000Z" }),
        ),
      );
      const [card] = cards();
      expect(within(card!).getByTestId("subagent-elapsed").textContent).toBe("1m 5s");
      act(() => void vi.advanceTimersByTime(2000));
      expect(within(card!).getByTestId("subagent-elapsed").textContent).toBe("1m 7s");
      await frame(
        event(3, "agent.tool_call_update", {
          tool_call_id: "clock1",
          title: null,
          status: "completed",
          content: null,
          subagent: { duration_ms: 70_000 },
        }),
      );
      expect(within(card!).queryByTestId("subagent-elapsed")).toBeNull();
      expect(within(card!).getByTestId("subagent-duration").textContent).toBe("1m 10s");
      act(() => void vi.advanceTimersByTime(5000));
      expect(within(card!).getByTestId("subagent-duration").textContent).toBe("1m 10s");
    } finally {
      vi.useRealTimers();
    }
    controller.dispose();
  });

  it("counts from when it first saw the launch when the Host does not say, across a remount", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "Date"] });
    try {
      vi.setSystemTime(new Date("2026-10-24T10:00:00Z"));
      const opened = await openSession(new FakeTransport());
      await opened.frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot({ status: "prompting" }) });
      await opened.frame(event(2, "agent.tool_call", launch("clock2", { description: "Survey" })));
      // A start ahead of this clock counts as just started.
      await opened.frame(event(3, "agent.tool_call", launch("clock3", { started_at: "2026-10-24T10:05:00.000Z" })));
      expect(screen.getAllByTestId("subagent-elapsed").map((e) => e.textContent)).toEqual(["0s", "0s"]);
      act(() => void vi.advanceTimersByTime(3000));
      expect(within(cards()[0]!).getByTestId("subagent-elapsed").textContent).toBe("3s");
      opened.controller.dispose();
      cleanup();

      // Back to the session: the clock goes on from the first sight, not from zero.
      vi.setSystemTime(new Date("2026-10-24T10:00:10Z"));
      const again = await openSession(new FakeTransport());
      await again.frame({
        type: "snapshot",
        epoch: "e",
        event_seq: 9,
        snapshot: snapshot({
          status: "prompting",
          live_message: {
            message_id: null,
            role: "assistant",
            content: [{ type: "tool_call", tool_call_id: "clock2" }],
          },
          tool_calls: [launch("clock2", { description: "Survey" })],
        }),
      });
      expect(within(cards()[0]!).getByTestId("subagent-elapsed").textContent).toBe("10s");
      again.controller.dispose();
    } finally {
      vi.useRealTimers();
    }
  });

  it("renders the same from a snapshot after a reconnect", async () => {
    const t = new FakeTransport();
    const { controller, frame } = await openSession(t);
    await frame({
      type: "snapshot",
      epoch: "e",
      event_seq: 9,
      snapshot: snapshot({
        status: "prompting",
        live_message: {
          message_id: null,
          role: "assistant",
          content: [{ type: "text", text: "Delegating." }, { type: "tool_call", tool_call_id: "a1" }],
          subagents: [{ parent_id: "a1", blocks: [{ type: "text", text: "child found it" }, { type: "tool_call", tool_call_id: "r1" }] }],
        },
        tool_calls: [
          launch("a1", { description: "Map client add-workspace code", model: "claude-sonnet-5-5", prompt: "Find it" }),
          { tool_call_id: "r1", title: "Read src/a.ts", kind: "read", status: "completed", content: null, parent_id: "a1" },
        ],
      }),
    });
    const [card] = cards();
    expect(cards()).toHaveLength(1);
    expect(within(card!).getByTestId("subagent-name").textContent).toBe("Agent • Sonnet 5.5");
    expect(within(card!).getByTestId("subagent-tools").textContent).toBe("1 tool");
    expect(screen.queryByText("child found it")).toBeNull();
    fireEvent.click(row(card!));
    expect(within(card!).getByTestId("markdown").textContent).toBe("child found it");
    expect(within(card!).getByTestId("tool-call").textContent).toBe("Reada.tsdone");
    controller.dispose();
  });

  it("renders from history, with a sub-agent's own sub-agents nested as cards", async () => {
    const t = new FakeTransport();
    t.handlers["session.history"] = (p) => ({
      session_id: p.session_id,
      turns: [
        turn("t1", "assistant", [
          {
            type: "tool_call",
            tool_call_id: "a1",
            title: "Task",
            input_preview: null,
            status: null,
            subagent: {
              description: "Survey the repo",
              model: "claude-opus-4-1-20250805",
              duration_ms: 42_000,
              tokens: 75_223,
              blocks: [
                { type: "text", text: "surveying" },
                { type: "tool_call", tool_call_id: "b1", title: "Bash", input_preview: JSON.stringify({ command: "ls" }), status: null },
                { type: "tool_result", tool_call_id: "b1", output_preview: "a.ts", is_error: false },
                { type: "tool_call", tool_call_id: "a2", title: "Task", input_preview: null, status: "completed", subagent: { description: "Dig deeper", blocks: [] } },
              ],
            },
          },
          { type: "tool_result", tool_call_id: "a1", output_preview: "done", is_error: false },
          { type: "text", text: "All done." },
        ]),
      ],
    });
    const { controller } = await openSession(t);
    const [card] = cards();
    expect(cards()).toHaveLength(1);
    expect(card!.getAttribute("data-status")).toBe("completed");
    expect(within(card!).getByTestId("subagent-name").textContent).toBe("Agent • Opus 4.1");
    expect(within(card!).getByTestId("subagent-duration").textContent).toBe("42s");
    expect(within(card!).getByTestId("subagent-tools").textContent).toBe("2 tools");
    expect(within(card!).getByTestId("subagent-tokens").textContent).toBe("75k tokens");
    expect(screen.queryByText("surveying")).toBeNull();
    fireEvent.click(row(card!));
    const body = within(card!).getByTestId("subagent-body");
    expect(within(body).getByText("surveying")).toBeTruthy();
    expect(within(body).getByTestId("tool-call").textContent).toContain("Run");
    const nested = within(body).getAllByTestId("subagent-card");
    expect(nested).toHaveLength(1);
    expect(within(nested[0]!).getByTestId("subagent-description").textContent).toBe("Dig deeper");
    expect(screen.getByText("All done.")).toBeTruthy();
    controller.dispose();
  });

  it("reads a sub-agent its finished turn never heard back from as interrupted", async () => {
    const t = new FakeTransport();
    t.handlers["session.history"] = (p) => ({
      session_id: p.session_id,
      turns: [
        turn("t1", "assistant", [
          { type: "tool_call", tool_call_id: "a1", title: "Task", input_preview: null, status: null, subagent: { description: "Survey", blocks: [] } },
        ]),
      ],
    });
    const { controller } = await openSession(t);
    const [card] = cards();
    expect(card!.getAttribute("data-status")).toBe("interrupted");
    expect(within(card!).getByTestId("subagent-interrupted").textContent).toBe("interrupted");
    expect(within(card!).queryByText("failed")).toBeNull();
    controller.dispose();
  });

  it("shows a background sub-agent's work after a reload under its card in the history", async () => {
    const t = new FakeTransport();
    t.handlers["session.history"] = (p) => ({
      session_id: p.session_id,
      turns: [
        turn("t1", "assistant", [
          { type: "tool_call", tool_call_id: "a1", title: "Task", input_preview: null, status: "completed", subagent: { description: "Review", blocks: [{ type: "text", text: "started" }] } },
          { type: "tool_result", tool_call_id: "a1", output_preview: "launched", is_error: false, background: null },
        ]),
      ],
    });
    const { controller, frame } = await openSession(t);
    await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot({ status: "connected" }) });
    await frame(event(2, "session.background", { outstanding: 1, tasks: [{ task_id: "t1", label: "Review", state: "running", tool_call_id: "a1" }], waiting_since: null }));
    await frame(event(3, "agent.subagent_message_delta", { parent_id: "a1", text: "found more" }));
    expect(cards()).toHaveLength(1);
    const [card] = cards();
    expect(card!.getAttribute("data-status")).toBe("in_progress");
    fireEvent.click(row(card!));
    const body = within(card!).getByTestId("subagent-body");
    expect(within(body).getByText("started")).toBeTruthy();
    expect(within(body).getByText("found more")).toBeTruthy();
    controller.dispose();
  });

  it("keeps an older Host's agent launch as a plain tool row", async () => {
    const t = new FakeTransport();
    t.handlers["session.history"] = (p) => ({
      session_id: p.session_id,
      turns: [
        turn("t1", "assistant", [
          { type: "tool_call", tool_call_id: "a1", title: "Task", input_preview: JSON.stringify({ description: "Survey the repo" }), status: null },
          { type: "tool_result", tool_call_id: "a1", output_preview: "done", is_error: false },
        ]),
      ],
    });
    const { controller } = await openSession(t);
    expect(cards()).toHaveLength(0);
    expect(screen.getByTestId("tool-call").textContent).toContain("Agent");
    controller.dispose();
  });

  it("says what the agent waits on and for how long, then clears", async () => {
    const t = new FakeTransport();
    const { controller, frame } = await openSession(t);
    await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot({ status: "connected" }) });
    expect(screen.queryByTestId("background-waiting")).toBeNull();
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "Date"] });
    try {
      vi.setSystemTime(new Date("2026-10-19T10:01:05Z"));
      const tasks = [{ task_id: "t1", label: "Review", state: "running" }];
      await frame(event(2, "session.background", { outstanding: 1, tasks, waiting_since: "2026-10-19T10:00:00Z" }));
      const banner = screen.getByTestId("background-waiting");
      expect(banner.textContent).toBe("Waiting on 1 background task1m 5s");
      act(() => void vi.advanceTimersByTime(2000));
      expect(screen.getByTestId("background-waiting-time").textContent).toBe("1m 7s");
      await frame(event(3, "session.background", { outstanding: 3, tasks, waiting_since: "2026-10-19T10:00:00Z" }));
      expect(screen.getByTestId("background-waiting").textContent).toContain("Waiting on 3 background tasks");
      // Running a turn again (no longer waiting), or nothing outstanding: gone.
      await frame(event(4, "session.background", { outstanding: 3, tasks, waiting_since: null }));
      expect(screen.queryByTestId("background-waiting")).toBeNull();
      await frame(event(5, "session.background", { outstanding: 1, tasks, waiting_since: "2026-10-19T10:00:00Z" }));
      expect(screen.getByTestId("background-waiting")).toBeTruthy();
      await frame(event(6, "session.background", { outstanding: 0, tasks: [], waiting_since: "2026-10-19T10:00:00Z" }));
      expect(screen.queryByTestId("background-waiting")).toBeNull();
    } finally {
      vi.useRealTimers();
    }
    controller.dispose();
  });

  it("nests a sub-agent's own sub-agents as cards in a live turn", async () => {
    const { controller, frame } = await twoSubagents();
    await frame(event(8, "agent.tool_call", launch("a3", { description: "Dig deeper" }, { parent_id: "a1" })));
    await frame(event(9, "agent.subagent_message_delta", { parent_id: "a3", text: "deep result" }));
    // Still two top-level cards; the nested one is inside the first, unmounted until opened.
    expect(cards()).toHaveLength(2);
    expect(screen.queryByText("deep result")).toBeNull();
    fireEvent.click(row(cards()[0]!));
    const nested = within(within(cards()[0]!).getByTestId("subagent-body")).getAllByTestId("subagent-card");
    expect(nested).toHaveLength(1);
    expect(within(nested[0]!).getByTestId("subagent-description").textContent).toBe("Dig deeper");
    expect(within(nested[0]!).queryByText("deep result")).toBeNull();
    fireEvent.click(row(nested[0]!));
    expect(within(nested[0]!).getByText("deep result")).toBeTruthy();
    controller.dispose();
  });

  it("puts a sub-agent whose launch is not in the reply last, so nothing it did is hidden", async () => {
    const t = new FakeTransport();
    const { controller, frame } = await openSession(t);
    await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot({ status: "prompting" }) });
    await frame(event(2, "agent.message_delta", { text: "Working on it." }));
    await frame(event(3, "agent.subagent_message_delta", { parent_id: "x9", text: "orphan says" }));
    await frame(event(4, "agent.tool_call", launch("a1", { description: "Review the diff" })));
    const [first, orphan] = cards();
    expect(cards()).toHaveLength(2);
    expect(within(first!).getByTestId("subagent-description").textContent).toBe("Review the diff");
    // Nothing is known of it but what it did: no doubled "Agent", no description.
    expect(within(orphan!).getByTestId("subagent-name").textContent).toBe("Agent");
    expect(within(orphan!).queryByTestId("subagent-description")).toBeNull();
    expect(screen.queryByText("orphan says")).toBeNull();
    fireEvent.click(row(orphan!));
    expect(within(orphan!).getByText("orphan says")).toBeTruthy();
    controller.dispose();
  });

  it("folds a sub-agent whose launch is not in the reply with the run it follows", async () => {
    const t = new FakeTransport();
    const { controller, frame } = await openSession(t);
    await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot({ status: "prompting" }) });
    await frame(
      event(2, "agent.tool_call", {
        tool_call_id: "c1",
        title: "Read a.ts",
        kind: "read",
        status: "completed",
        content: null,
      }),
    );
    await frame(
      event(3, "agent.tool_call", {
        tool_call_id: "c2",
        title: "Read b.ts",
        kind: "read",
        status: "completed",
        content: null,
      }),
    );
    await frame(event(4, "agent.subagent_message_delta", { parent_id: "x9", text: "orphan says" }));
    const run = screen.getByTestId("tool-run");
    expect(within(run).getAllByRole("button")[0]!.textContent).toContain("2 tool calls, 1 sub-agent");
    fireEvent.click(within(run).getAllByRole("button")[0]!);
    expect(within(run).getAllByTestId("subagent-card")).toHaveLength(1);
    controller.dispose();
  });

  it("keeps a sub-agent opened live open once the history has its turn", async () => {
    const t = new FakeTransport();
    const user = turn("u1", "user", [{ type: "text", text: "go" }]);
    const launched = (id: string, description: string): HistoryTurn["blocks"][number] => ({
      type: "tool_call",
      tool_call_id: id,
      title: "Task",
      input_preview: null,
      status: "completed",
      subagent: { description, blocks: [] },
    });
    const first = launched("h1", "Survey");
    const said = { type: "text", text: "Surveyed." } as const;
    t.handlers["session.history"] = (p) => ({
      session_id: p.session_id,
      turns: [user, turn("a1", "assistant", [first, said])],
    });
    const { controller, frame } = await openSession(t);
    // The history holds the turn the agent is still running; it launches another.
    await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot({ status: "prompting" }) });
    await frame(event(2, "agent.tool_call", launch("a2", { description: "Review the diff" })));
    expect(cards()).toHaveLength(2);
    fireEvent.click(row(cards()[1]!));
    expect(row(cards()[1]!).getAttribute("aria-expanded")).toBe("true");

    t.handlers["session.history"] = (p) => ({
      session_id: p.session_id,
      turns: [user, turn("a1", "assistant", [first, said, launched("a2", "Review the diff")])],
    });
    await frame(event(3, "agent.turn_complete", { stop_reason: "end_turn" }));
    await act(() => tick());
    const [kept, opened] = cards();
    expect(cards()).toHaveLength(2);
    expect(
      screen.queryAllByTestId("live-turn").flatMap((l) => within(l).queryAllByTestId("subagent-card")),
    ).toHaveLength(0);
    expect(row(kept!).getAttribute("aria-expanded")).toBe("false");
    expect(row(opened!).getAttribute("aria-expanded")).toBe("true");
    controller.dispose();
  });

  // A new turn that reads a file and launches a sub-agent, both opened live;
  // and the history once it has the turn.
  async function openNewTurn() {
    const t = new FakeTransport();
    const before = [
      turn("u1", "user", [{ type: "text", text: "go" }]),
      turn("a1", "assistant", [{ type: "text", text: "Done." }]),
    ];
    t.handlers["session.history"] = (p) => ({ session_id: p.session_id, turns: before });
    const { controller, frame } = await openSession(t);
    await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot({ status: "connected" }) });
    await frame(event(2, "agent.user_message", { message_id: "m2", blocks: [{ type: "text", text: "review" }] }));
    await frame(
      event(3, "agent.tool_call", {
        tool_call_id: "c1",
        title: "Read a.ts",
        kind: "read",
        status: "completed",
        content: "out",
      }),
    );
    await frame(event(4, "agent.tool_call", launch("x1", { description: "Review the diff" })));
    const call = () => within(screen.getAllByTestId("tool-call").at(-1)!).getAllByRole("button")[0]!;
    fireEvent.click(call());
    fireEvent.click(row(cards()[0]!));
    expect(call().getAttribute("aria-expanded")).toBe("true");
    expect(row(cards()[0]!).getAttribute("aria-expanded")).toBe("true");
    const after = [
      ...before,
      turn("u2", "user", [{ type: "text", text: "review" }]),
      turn("a2", "assistant", [
        {
          type: "tool_call",
          tool_call_id: "c1",
          title: "Read",
          input_preview: '{"file_path":"a.ts"}',
          status: "completed",
        },
        { type: "tool_result", tool_call_id: "c1", output_preview: "out", is_error: false },
        {
          type: "tool_call",
          tool_call_id: "x1",
          title: "Task",
          input_preview: null,
          status: "completed",
          subagent: { description: "Review the diff", blocks: [] },
        },
      ]),
    ];
    return { t, controller, frame, call, after };
  }

  it("keeps a sub-agent and a call opened in a new turn open once the history has the turn", async () => {
    const { t, controller, frame, call, after } = await openNewTurn();
    t.handlers["session.history"] = (p) => ({ session_id: p.session_id, turns: after });
    await frame(event(5, "agent.turn_complete", { stop_reason: "end_turn" }));
    await act(() => tick());
    expect(screen.queryAllByTestId("live-turn").flatMap((l) => within(l).queryAllByTestId("tool-call"))).toHaveLength(
      0,
    );
    expect(screen.getByText("review")).toBeTruthy();
    expect(call().getAttribute("aria-expanded")).toBe("true");
    expect(row(cards()[0]!).getAttribute("aria-expanded")).toBe("true");
    controller.dispose();
  });

  it("keeps what a new turn opened open when the turn ended while away", async () => {
    const { t, controller, frame, call, after } = await openNewTurn();
    // Back after the turn ended: the snapshot has no live turn, and the
    // history on screen is behind until the fresh read arrives.
    let read!: () => void;
    t.handlers["session.history"] = (p) =>
      new Promise((done) => (read = () => done({ session_id: p.session_id, turns: after })));
    await frame({ type: "snapshot", epoch: "e2", event_seq: 9, snapshot: snapshot({ status: "connected" }) });
    expect(screen.queryAllByTestId("tool-call")).toHaveLength(0);
    await act(async () => read());
    await act(() => tick());
    expect(screen.getByText("review")).toBeTruthy();
    expect(call().getAttribute("aria-expanded")).toBe("true");
    expect(row(cards()[0]!).getAttribute("aria-expanded")).toBe("true");
    controller.dispose();
  });

  it("describes a sub-agent by its launch's title only when that says more than Agent", async () => {
    const t = new FakeTransport();
    const { controller, frame } = await openSession(t);
    await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot({ status: "prompting" }) });
    await frame(event(2, "agent.tool_call", launch("a1", {})));
    await frame(event(3, "agent.tool_call", launch("a2", {}, { title: "`Survey the repo`" })));
    const [generic, titled] = cards();
    expect(within(generic!).queryByTestId("subagent-description")).toBeNull();
    expect(generic!.textContent).not.toContain("AgentAgent");
    expect(within(titled!).getByTestId("subagent-description").textContent).toBe("Survey the repo");
    controller.dispose();
  });

  it("shows the images its launch returned after what it did, live, but not the launch's text", async () => {
    const { controller, frame } = await twoSubagents();
    await frame(event(8, "agent.tool_call_update", {
      tool_call_id: "a1",
      title: null,
      status: "completed",
      content: "child found it (as the launch's result)",
      images: [{ name: "shot.png", mime_type: "image/png", path: ".zwiim/images/shot.png" }],
    }));
    const [first] = cards();
    expect(screen.queryAllByTestId("agent-image")).toHaveLength(0);
    fireEvent.click(row(first!));
    const body = within(first!).getByTestId("subagent-body");
    const images = within(body).getByTestId("subagent-images");
    expect(within(images).getByTestId("agent-image").textContent).toContain("shot.png");
    // After the sub-agent's own rows.
    expect(body.lastElementChild).toBe(images);
    expect(screen.queryByText("child found it (as the launch's result)")).toBeNull();
    controller.dispose();
  });

  it("shows the images its launch returned after what it did, from history, but not the launch's text", async () => {
    const t = new FakeTransport();
    t.handlers["session.history"] = (p) => ({
      session_id: p.session_id,
      turns: [
        turn("t1", "assistant", [
          { type: "tool_call", tool_call_id: "a1", title: "Task", input_preview: null, status: null, subagent: { description: "Draw it", blocks: [{ type: "text", text: "drew it" }] } },
          {
            type: "tool_result",
            tool_call_id: "a1",
            output_preview: "the launch's own text",
            is_error: false,
            images: [{ name: "plot.png", mime_type: "image/png", path: ".zwiim/images/plot.png" }],
          },
        ]),
      ],
    });
    const { controller } = await openSession(t);
    const [card] = cards();
    expect(screen.queryAllByTestId("agent-image")).toHaveLength(0);
    fireEvent.click(row(card!));
    const body = within(card!).getByTestId("subagent-body");
    expect(within(body).getByText("drew it")).toBeTruthy();
    expect(body.lastElementChild).toBe(within(body).getByTestId("subagent-images"));
    expect(within(body).getByTestId("agent-image").textContent).toContain("plot.png");
    expect(screen.queryByText("the launch's own text")).toBeNull();
    controller.dispose();
  });

  it("announces how many tasks it waits on, not each second", async () => {
    const t = new FakeTransport();
    const { controller, frame } = await openSession(t);
    await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot({ status: "connected" }) });
    const tasks = [{ task_id: "t1", label: "Review", state: "running" }];
    await frame(event(2, "session.background", { outstanding: 2, tasks, waiting_since: "2026-10-19T10:00:00Z" }));
    const banner = screen.getByRole("status");
    expect(banner).toBe(screen.getByTestId("background-waiting"));
    expect(screen.getByTestId("background-waiting-time").getAttribute("aria-hidden")).toBe("true");
    // What a screen reader reads: the count alone.
    const read = [...banner.childNodes]
      .filter((n) => !(n instanceof HTMLElement && n.getAttribute("aria-hidden") === "true"))
      .map((n) => n.textContent)
      .join("");
    expect(read).toBe("Waiting on 2 background tasks");
    controller.dispose();
  });

  it("shows no waiting banner for a time it cannot read", async () => {
    const t = new FakeTransport();
    const { controller, frame } = await openSession(t);
    await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot({ status: "connected" }) });
    const tasks = [{ task_id: "t1", label: "Review", state: "running" }];
    await frame(event(2, "session.background", { outstanding: 1, tasks, waiting_since: "not a time" }));
    expect(screen.queryByTestId("background-waiting")).toBeNull();
    await frame(event(3, "session.background", { outstanding: 1, tasks, waiting_since: "2026-10-19T10:00:00Z" }));
    expect(screen.getByTestId("background-waiting")).toBeTruthy();
    controller.dispose();
  });

  it("reads a sub-agent running while its background task is outstanding", async () => {
    const t = new FakeTransport();
    const { controller, frame } = await openSession(t);
    await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot({ status: "prompting" }) });
    await frame(event(2, "agent.tool_call", launch("a1", { description: "Review" }, { status: "completed" })));
    expect(cards()[0]!.getAttribute("data-status")).toBe("completed");
    await frame(event(3, "session.background", { outstanding: 1, tasks: [{ task_id: "t1", label: "Review", state: "running", tool_call_id: "a1" }], waiting_since: null }));
    expect(cards()[0]!.getAttribute("data-status")).toBe("in_progress");
    expect(within(cards()[0]!).getByTestId("subagent-elapsed")).toBeTruthy();
    controller.dispose();
  });
});

describe("links", () => {
  it("shortens bare web addresses only", () => {
    expect(linkText("https://github.com/a/b/", "https://github.com/a/b/")).toBe("github.com/a/b");
    expect(linkText("https://github.com/a/b", ["https://github.com/a/b"])).toBe("github.com/a/b");
    expect(linkText("https://github.com/a/b", "the PR")).toBe("the PR");
    expect(linkText("mailto:a@b.c", "mailto:a@b.c")).toBe("mailto:a@b.c");
  });

  it("opens web links in the browser and keeps the app where it is", async () => {
    const opened = vi.spyOn(window, "open").mockReturnValue(null);
    const t = new FakeTransport();
    t.handlers["session.history"] = (p) => ({
      session_id: p.session_id,
      turns: [turn("t1", "assistant", [{ type: "text", text: "See https://example.com/docs and [this](file:///etc/passwd)." }])],
    });
    const { controller } = await openSession(t);
    // A bare URL reads without its scheme; the full address stays in the tooltip.
    const web = screen.getByRole("link", { name: "example.com/docs" });
    expect(web.getAttribute("title")).toBe("https://example.com/docs");
    expect(web.querySelector("svg")).not.toBeNull();
    const file = screen.getByText("this");
    expect(file.closest("a")!.querySelector("svg")).toBeNull();
    expect(fireEvent.click(web!)).toBe(false);
    await tick();
    expect(opened).toHaveBeenCalledWith("https://example.com/docs", "_blank", "noopener,noreferrer");
    expect(fireEvent.click(file!)).toBe(false);
    await tick();
    expect(opened).toHaveBeenCalledTimes(1);
    opened.mockRestore();
    controller.dispose();
  });
});

describe("file links", () => {
  const worktree = {
    workspace_id: "w1",
    name: "a9b70af5",
    alias: null,
    kind: "worktree",
    parent_workspace_id: "root",
    is_git_repo: true,
    branch: "zwiim/a9b70af5",
    base_branch: "main",
    base_commit: null,
    removed: false,
    display_path: null,
  } as const;
  const root = { ...worktree, name: "zwiim", kind: "root", parent_workspace_id: null, display_path: "~/Developer/zwiim" } as const;

  it("resolves a link to a file in the workspace, and nothing outside it", () => {
    expect(workspaceFile("docs/x.md", worktree)).toBe("docs/x.md");
    expect(workspaceFile("./src/a.ts:12", worktree)).toBe("src/a.ts");
    expect(workspaceFile("src/a.ts#L12", worktree)).toBe("src/a.ts");
    expect(workspaceFile("/Users/me/zwiim/worktrees/zwiim/a9b70af5/docs/x.md", worktree)).toBe("docs/x.md");
    expect(workspaceFile("file:///Users/me/zwiim/worktrees/zwiim/a9b70af5/my%20notes.md", worktree)).toBe("my notes.md");
    // A root is found by its home-relative path, not a folder of the same name inside it.
    expect(workspaceFile("/Users/me/Developer/zwiim/clients/zwiim/a.ts", root)).toBe("clients/zwiim/a.ts");
    expect(workspaceFile("~/Developer/zwiim/README.md", root)).toBe("README.md");
    expect(workspaceFile("/Users/me/other/README.md", root)).toBeNull();
    expect(workspaceFile("/Users/me/zwiim/worktrees/zwiim/b5a7d5b8/x.md", worktree)).toBeNull();
    expect(workspaceFile("../secrets.txt", worktree)).toBeNull();
    expect(workspaceFile("https://example.com/a.md", worktree)).toBeNull();
    expect(workspaceFile("mailto:a@b.c", worktree)).toBeNull();
    expect(workspaceFile("#heading", worktree)).toBeNull();
  });

  it("opens a workspace file beside the conversation; other files stay inert", () => {
    const open = vi.fn();
    render(
      <FileLinksContext.Provider value={{ workspace: worktree, open, openDiff: vi.fn() }}>
        <Markdown
          text={"[x.md](/Users/me/zwiim/worktrees/zwiim/a9b70af5/docs/x.md) and [y](/etc/passwd)"}
        />
      </FileLinksContext.Provider>,
    );
    const file = screen.getByRole("link", { name: "x.md" });
    expect(file.getAttribute("title")).toBe("docs/x.md");
    expect(fireEvent.click(file)).toBe(false);
    expect(open).toHaveBeenCalledWith("docs/x.md");
    fireEvent.click(screen.getByRole("link", { name: "y" }));
    expect(open).toHaveBeenCalledTimes(1);
    cleanup();

    // Without a workspace to open in (a removed worktree), file links stay inert.
    render(<Markdown text="[x.md](docs/x.md)" />);
    expect(screen.getByRole("link", { name: "x.md" }).className).toContain("cursor-default");
  });
});

describe("request cards", () => {
  it("shows what a permission would run or change", () => {
    const request: AgentRequest = {
      kind: "permission",
      request_id: "r",
      title: "Bash",
      tool: {
        tool_call_id: null,
        kind: "execute",
        text: null,
        command: "rm -rf build",
        diffs: [{ path: "src/a.rs", old_text: "a\n", new_text: "b\n" }],
      },
      plan_markdown: null,
      options: [{ option_id: "y", name: "Allow", kind: "allow_once" }],
    };
    render(<RequestCard request={request} onAnswer={vi.fn(async () => {})} />);
    const tool = screen.getByTestId("permission-tool");
    expect(tool.textContent).toContain("$ rm -rf build");
    expect(tool.textContent).toContain("src/a.rs");
    expect(tool.textContent).toContain("-a");
    expect(tool.textContent).toContain("+b");
  });

  it("asks about a plan the transcript already shows without repeating it", () => {
    const request: AgentRequest = {
      kind: "permission",
      request_id: "r",
      title: "Implement this plan?",
      tool: { tool_call_id: "plan-review:p1", kind: "switch_mode", text: null, command: null, diffs: [] },
      plan_markdown: null,
      options: [{ option_id: "y", name: "Yes, implement this plan", kind: "allow_once" }],
    };
    render(<RequestCard request={request} onAnswer={vi.fn(async () => {})} />);
    const card = screen.getByTestId("request-card");
    expect(card.textContent).toContain("The agent has a plan");
    expect(card.textContent).toContain("Implement this plan?");
    expect(screen.queryByTestId("plan")).toBeNull();
  });

  it("renders a plan as markdown with the agent's own choices", async () => {
    const onAnswer = vi.fn(async () => {});
    const request: AgentRequest = {
      kind: "permission",
      request_id: "r",
      title: "Ready to code?",
      tool: null,
      plan_markdown: "## Plan\n\n1. Do the thing",
      options: [
        { option_id: "exit-plan-default", name: "Yes, and manually approve edits", kind: "allow_once" },
        { option_id: "plan", name: "No, keep planning", kind: "reject_once" },
      ],
    };
    render(<RequestCard request={request} onAnswer={onAnswer} />);
    expect(screen.getByTestId("plan").querySelector("h2")?.textContent).toBe("Plan");
    fireEvent.click(screen.getByText("No, keep planning"));
    await tick();
    expect(onAnswer).toHaveBeenCalledWith({ kind: "permission", answer: { option_id: "plan" } });
  });

  it("confirms before abandoning a plan", async () => {
    const onAnswer = vi.fn(async () => {});
    render(
      <RequestCard request={{ kind: "plan_approval", request_id: "p", plan_markdown: "- step" }} onAnswer={onAnswer} />,
    );
    fireEvent.click(screen.getByText("Abandon"));
    expect(onAnswer).not.toHaveBeenCalled();
    fireEvent.click(screen.getByText("Keep"));
    fireEvent.click(screen.getByText("Abandon"));
    fireEvent.click(screen.getByText("Discard"));
    await tick();
    expect(onAnswer).toHaveBeenCalledWith({ kind: "plan_approval", answer: { decision: "abandon" } });
  });

  it("sends typed plan adjustments in place of approving", async () => {
    const onAnswer = vi.fn(async () => {});
    render(
      <RequestCard request={{ kind: "plan_approval", request_id: "p", plan_markdown: "- step" }} onAnswer={onAnswer} />,
    );
    expect(screen.queryByText("Send adjustments")).toBeNull();
    fireEvent.change(screen.getByPlaceholderText("Enter your plan adjustments here…"), {
      target: { value: "  use sqlite  " },
    });
    expect(screen.queryByText("Approve")).toBeNull();
    fireEvent.click(screen.getByText("Send adjustments"));
    await tick();
    expect(onAnswer).toHaveBeenCalledWith({
      kind: "plan_approval",
      answer: { decision: "request_changes", feedback: "use sqlite" },
    });
  });

  it("copies the plan", async () => {
    const writeText = vi.fn(async () => {});
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    render(
      <RequestCard request={{ kind: "plan_approval", request_id: "p", plan_markdown: "- step" }} onAnswer={vi.fn()} />,
    );
    fireEvent.click(screen.getByText("Copy"));
    await tick();
    expect(writeText).toHaveBeenCalledWith("- step");
    expect(screen.getByText("Copied")).toBeTruthy();
  });

  it("offers hand off only when the Host can", async () => {
    const onAnswer = vi.fn(async () => {});
    const request: AgentRequest = { kind: "plan_approval", request_id: "p", plan_markdown: "- step" };
    const { rerender } = render(<RequestCard request={request} onAnswer={onAnswer} />);
    expect(screen.queryByText("Hand off")).toBeNull();
    rerender(<RequestCard request={request} onAnswer={onAnswer} handOff />);
    fireEvent.click(screen.getByText("Hand off"));
    await tick();
    expect(onAnswer).toHaveBeenCalledWith({ kind: "plan_approval", answer: { decision: "hand_off" } });
  });
});

describe("error wording", () => {
  it("explains technical failures plainly and keeps specific host messages", () => {
    expect(errorMessage(new ZwiimClientError("turn_in_progress", "turn in flight", 409))).toMatch(/still working/);
    expect(errorMessage(new ZwiimClientError("workspace_removed", "The workspace was removed", 409))).toBe(
      "The workspace was removed",
    );
    expect(errorMessage(new ZwiimClientError("internal", "db locked", 500))).toBe(
      "Something went wrong on the host: db locked",
    );
  });
});
