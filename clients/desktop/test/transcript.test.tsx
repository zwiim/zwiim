import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  HostClient,
  HostController,
  ZwiimClientError,
  type AgentRequest,
  type HistoryTurn,
  type SessionSnapshot,
} from "@zwiim/sdk";
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

async function openSession(t: FakeTransport) {
  const client = new HostClient({ transport: t });
  const controller = new HostController(client);
  client.start();
  await tick();
  render(
    <HostProvider value={controller}>
      <SessionPanel workspaceId="w1" sessionId="s1" onSessionStarted={() => {}} />
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

  it("shows a tool call with its status, input and output", async () => {
    const t = new FakeTransport();
    t.handlers["session.history"] = (p) => ({
      session_id: p.session_id,
      turns: [
        turn("t1", "assistant", [
          { type: "tool_call", tool_call_id: "c1", title: "Bash", input_preview: "ls src", status: null },
          { type: "tool_result", tool_call_id: "c1", output_preview: "main.rs", is_error: false },
          { type: "tool_call", tool_call_id: "c2", title: "Read", input_preview: "x.txt", status: "completed" },
          { type: "tool_result", tool_call_id: "c2", output_preview: "no such file", is_error: true },
        ]),
      ],
    });
    const { controller } = await openSession(t);
    const calls = screen.getAllByTestId("tool-call");
    expect(calls).toHaveLength(2);
    expect(calls[0]!.textContent).toContain("Bash");
    expect(calls[0]!.textContent).toContain("done");
    expect(calls[0]!.textContent).toContain("ls src");
    expect(calls[0]!.textContent).toContain("main.rs");
    expect(calls[1]!.getAttribute("data-status")).toBe("failed");
    // The result is shown with its call, not again on its own.
    expect(screen.getAllByText("no such file")).toHaveLength(1);
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
    await frame({
      type: "event",
      event: { session_id: "s1", seq: 3, type: "agent.tool_call_update", payload: { tool_call_id: "k", title: null, status: "completed", content: "--- a.txt\n+++ a.txt\n-old\n+new\n" } },
    });
    const call = screen.getByTestId("tool-call");
    expect(call.textContent).toContain("done");
    expect([...call.querySelectorAll("pre div")].map((d) => d.textContent)).toContain("+new");
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
    const box = screen.getByPlaceholderText("Send a message");
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
    fireEvent.click(screen.getByText("Jump to latest"));
    expect(el.scrollTop).toBe(1000);
    expect(screen.queryByText("Jump to latest")).toBeNull();
    controller.dispose();
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
