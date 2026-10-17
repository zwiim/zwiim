import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  clockTime,
  HostClient,
  HostController,
  ZwiimClientError,
  type AgentRequest,
  type HistoryTurn,
  type SessionSnapshot,
} from "@zwiim/sdk";
import { FileLinksContext, linkText, Markdown, workspaceFile, type FileLinks } from "@/components/Markdown";
import { errorMessage, HostProvider } from "@/lib/host-context";
import { RequestCard } from "@/panels/RequestCard";
import { SessionPanel } from "@/panels/SessionPanel";
import { FakeTransport, tick } from "./fakes";

afterEach(cleanup);

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

async function openSession(t: FakeTransport, links?: FileLinks) {
  const client = new HostClient({ transport: t });
  const controller = new HostController(client);
  client.start();
  await tick();
  const panel = <SessionPanel workspaceId="w1" sessionId="s1" onSessionStarted={() => {}} />;
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
        ]),
      ],
    });
    const { controller } = await openSession(t);
    const calls = screen.getAllByTestId("tool-call");
    expect(calls).toHaveLength(3);
    // Named as Claude titles it live.
    expect(calls[2]!.textContent).toContain("Exited Plan Mode");
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
    t.handlers["session.send"] = (p) => ({ session_id: p.session_id });
    const { controller, frame } = await openSession(t);
    await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot() });
    const box = screen.getByPlaceholderText(/^Send a message/);
    fireEvent.change(box, { target: { value: "hello" } });
    fireEvent.keyDown(box, { key: "Enter" });
    await tick();
    const sent = t.calls.find((c) => c.op === "session.send")!.params;
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
