import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { HostClient, HostController, ZwiimClientError } from "@zwiim/sdk";
import { bootHost, type BootPhase } from "@/lib/boot";
import type { DiscoveredHost, NativeBridge } from "@/lib/bridge";
import { HostProvider } from "@/lib/host-context";
import type { PendingFile } from "@/attachments/pending";
import { keepForkedDraft } from "@/panels/forkedDraft";
import { RequestCard } from "@/panels/RequestCard";
import { SessionPanel } from "@/panels/SessionPanel";
import { InputQueue } from "@/panels/TerminalPanel";

import { FakeTransport, tick } from "./fakes";

afterEach(cleanup);

const host: DiscoveredHost = { host_id: "h1", url: "http://127.0.0.1:1", token: "t", pid: 7, running: true };

function bridge(overrides: Partial<NativeBridge> = {}): NativeBridge {
  return {
    discover: vi.fn(async () => host),
    startHost: vi.fn(async () => {}),
    pickFolder: vi.fn(async () => null),
    accountStatus: vi.fn(async () => ({ available: false, account: null })),
    signIn: vi.fn(async () => ({ user_id: "u1", email: "me@example.com" })),
    cancelSignIn: vi.fn(async () => {}),
    signOut: vi.fn(async () => {}),
    ...overrides,
  };
}

describe("boot", () => {
  it("connects to a running host without starting one", async () => {
    const b = bridge();
    const t = new FakeTransport();
    const phases: string[] = [];
    const c = await bootHost(b, (p) => phases.push(p.phase), { makeTransport: () => t });
    expect(phases).toEqual(["discovering", "ready"]);
    expect(b.startHost).not.toHaveBeenCalled();
    c?.dispose();
  });

  it("starts the host when the discovery file is missing or stale, then connects", async () => {
    let n = 0;
    const b = bridge({
      discover: vi.fn(async () => {
        n++;
        if (n === 1) return { ...host, pid: 1, running: false }; // stale file
        if (n === 2) return { ...host, pid: 1, running: false }; // not written yet
        return { ...host, pid: 99 };
      }),
    });
    const t = new FakeTransport();
    const phases: BootPhase["phase"][] = [];
    const c = await bootHost(b, (p) => phases.push(p.phase), { makeTransport: () => t, pollMs: 1 });
    expect(phases).toEqual(["discovering", "starting", "ready"]);
    expect(b.startHost).toHaveBeenCalledTimes(1);
    c?.dispose();
  });

  it("reports a host that cannot be started", async () => {
    const b = bridge({
      discover: vi.fn(async () => null),
      startHost: vi.fn(async () => {
        throw new Error("Cannot start host (zwiim-host): not found");
      }),
    });
    const phases: BootPhase[] = [];
    expect(await bootHost(b, (p) => phases.push(p))).toBeNull();
    expect(phases.at(-1)).toEqual({ phase: "failed", message: "Cannot start host (zwiim-host): not found" });
  });
});

describe("terminal input queue", () => {
  it("sends in order, one request at a time, coalescing typed input", async () => {
    const sent: string[] = [];
    let release!: () => void;
    const send = vi.fn(
      (d: string) =>
        new Promise<void>((r) => {
          sent.push(d);
          release = r;
        }),
    );
    const q = new InputQueue(send, () => {});
    q.push("l");
    q.push("s");
    q.push("\r");
    expect(sent).toEqual(["l"]);
    release();
    await tick();
    expect(sent).toEqual(["l", "s\r"]);
  });

  it("does not resend a failed write", async () => {
    const send = vi.fn(async () => {
      throw new ZwiimClientError("unknown_outcome", "lost");
    });
    const onError = vi.fn();
    const q = new InputQueue(send, onError);
    q.push("rm -rf build\r");
    await tick();
    expect(send).toHaveBeenCalledTimes(1);
    expect(onError).toHaveBeenCalledTimes(1);
  });
});

describe("RequestCard", () => {
  it("answers a permission request with the chosen option", async () => {
    const onAnswer = vi.fn(async () => {});
    render(
      <RequestCard
        request={{
          kind: "permission",
          request_id: "r1",
          title: "Run npm test",
          tool: null,
          plan_markdown: null,
          options: [
            { option_id: "allow", name: "Allow once", kind: "allow_once" },
            { option_id: "deny", name: "Reject", kind: "reject_once" },
          ],
        }}
        onAnswer={onAnswer}
      />,
    );
    fireEvent.click(screen.getByText("Allow once"));
    await tick();
    expect(onAnswer).toHaveBeenCalledWith({ kind: "permission", answer: { option_id: "allow" } });
  });

  it("answers and declines a question", async () => {
    const onAnswer = vi.fn(async () => {});
    render(
      <RequestCard
        request={{
          kind: "question",
          request_id: "q1",
          questions: [
            {
              question_id: "a",
              header: "Lang",
              question: "Which language?",
              multi_select: false,
              options: [
                { label: "Rust", description: "" },
                { label: "Go", description: "" },
              ],
            },
          ],
        }}
        onAnswer={onAnswer}
      />,
    );
    fireEvent.click(screen.getByText("Go"));
    fireEvent.click(screen.getByText("Answer"));
    await tick();
    expect(onAnswer).toHaveBeenLastCalledWith({
      kind: "question",
      answer: { answers: [{ question_id: "a", labels: ["Go"] }] },
    });
    fireEvent.click(screen.getByText("Decline"));
    await tick();
    expect(onAnswer).toHaveBeenLastCalledWith({ kind: "question", answer: { answers: [], declined: true } });
  });

  it("approves a plan", async () => {
    const onAnswer = vi.fn(async () => {});
    render(
      <RequestCard request={{ kind: "plan_approval", request_id: "p1", plan_markdown: "1. do it" }} onAnswer={onAnswer} />,
    );
    fireEvent.click(screen.getByText("Approve"));
    await tick();
    expect(onAnswer).toHaveBeenCalledWith({ kind: "plan_approval", answer: { decision: "approve" } });
  });
});

const pendingFile = (name: string): PendingFile => ({
  id: name,
  name,
  size: 1,
  mime_type: "text/plain",
  file: new Blob(["x"]),
  status: "ready",
});

describe("SessionPanel", () => {
  it("opens a fork with the draft left in its source", async () => {
    const t = new FakeTransport();
    t.handlers["session.history"] = (p) => ({ session_id: p.session_id, turns: [] });
    const client = new HostClient({ transport: t });
    const controller = new HostController(client);
    client.start();
    await tick();
    keepForkedDraft("s2", { text: "half a thought", files: [pendingFile("notes.txt")] });

    const panel = (id: string) => (
      <HostProvider value={controller}>
        <SessionPanel workspaceId="w1" sessionId={id} onSessionStarted={() => {}} />
      </HostProvider>
    );
    const { rerender } = render(panel("s2"));
    await tick();
    expect(screen.getByRole<HTMLTextAreaElement>("textbox").value).toBe("half a thought");
    expect(screen.getByText("notes.txt")).toBeTruthy();

    rerender(panel("s1"));
    await tick();
    expect(screen.getByRole<HTMLTextAreaElement>("textbox").value).toBe("");
    expect(screen.queryByText("notes.txt")).toBeNull();
    rerender(panel("s2"));
    await tick();
    expect(screen.getByRole<HTMLTextAreaElement>("textbox").value).toBe("");
    expect(screen.queryByText("notes.txt")).toBeNull();
  });

  it("opens the session a plan was handed off to", async () => {
    const t = new FakeTransport();
    t.handlers["session.history"] = (p) => ({ session_id: p.session_id, turns: [] });
    t.handlers["session.respond"] = () => ({ session_id: "s2" });
    const client = new HostClient({ transport: t });
    const controller = new HostController(client);
    client.start();
    await tick();
    const onSessionStarted = vi.fn();

    render(
      <HostProvider value={controller}>
        <SessionPanel workspaceId="w1" sessionId="s1" onSessionStarted={onSessionStarted} />
      </HostProvider>,
    );
    await tick();
    await act(async () => {
      t.frame({
        type: "snapshot",
        subscription_id: "sub-1",
        session_id: "s1",
        epoch: "e1",
        event_seq: 1,
        snapshot: {
          status: "prompting",
          live_message: null,
          tool_calls: [],
          pending_request: { kind: "plan_approval", request_id: "p1", plan_markdown: "1. do it" },
          pending_user_message: null,
          last_error: null,
          usage: null,
          options: null,
          commands: [],
        },
      });
    });
    await tick();

    fireEvent.click(screen.getByText("Hand off"));
    await tick();
    expect(t.calls.find((c) => c.op === "session.respond")?.params).toEqual({
      session_id: "s1",
      request_id: "p1",
      kind: "plan_approval",
      answer: { decision: "hand_off" },
    });
    expect(onSessionStarted).toHaveBeenCalledWith("s2");
    controller.dispose();
  });

  it("renders history plus the live turn, and routes request answers to the host", async () => {
    const t = new FakeTransport();
    t.handlers["session.history"] = (p) => ({
      session_id: p.session_id,
      turns: [
        { turn_id: "t1", role: "user", blocks: [{ type: "text", text: "earlier question" }], timestamp: "", completed_at: null, duration_ms: null, model: null, usage: null },
        { turn_id: "t2", role: "assistant", blocks: [{ type: "text", text: "earlier answer" }], timestamp: "", completed_at: null, duration_ms: null, model: null, usage: null },
      ],
    });
    t.handlers["session.respond"] = () => ({});
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
    expect(t.sent).toContainEqual({ op: "session.subscribe", subscription_id: "sub-1", session_id: "s1" });

    await act(async () => {
      t.frame({
        type: "snapshot",
        subscription_id: "sub-1",
        session_id: "s1",
        epoch: "e1",
        event_seq: 3,
        snapshot: {
          status: "prompting",
          live_message: { message_id: "m", role: "assistant", content: [{ type: "text", text: "Working on" }] },
          tool_calls: [],
          pending_request: null,
          pending_user_message: { message_id: "u", blocks: [{ type: "text", text: "fix the bug" }] },
          last_error: null,
          usage: null,
          options: null,
          commands: [],
        },
      });
      t.frame({
        type: "event",
        subscription_id: "sub-1",
        event: { session_id: "s1", seq: 4, type: "agent.message_delta", payload: { text: " it" } },
      });
      t.frame({
        type: "event",
        subscription_id: "sub-1",
        event: {
          session_id: "s1",
          seq: 5,
          type: "agent.request",
          payload: { kind: "permission", request_id: "r9", title: "Edit main.rs", tool: null, plan_markdown: null, options: [{ option_id: "ok", name: "Allow", kind: "allow_once" }] },
        },
      });
    });
    await tick();

    const transcript = screen.getByTestId("transcript");
    expect(transcript.textContent).toContain("earlier question");
    expect(transcript.textContent).toContain("earlier answer");
    expect(transcript.textContent).toContain("fix the bug");
    expect(transcript.textContent).toContain("Working on it");
    expect(screen.getByLabelText("Stop agent")).toBeTruthy();

    fireEvent.click(screen.getByText("Allow"));
    await tick();
    expect(t.calls.find((c) => c.op === "session.respond")?.params).toEqual({
      session_id: "s1",
      request_id: "r9",
      kind: "permission",
      answer: { option_id: "ok" },
    });
    controller.dispose();
  });
});
