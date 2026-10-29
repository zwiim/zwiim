import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
  CLIENT_CAPABILITIES,
  HostClient,
  HostController,
  type QueuedFollowUp,
  type SessionSnapshot,
  type SessionSummary,
} from "@zwiim/sdk";
import { HostProvider } from "@/lib/host-context";
import { rememberFollowUpMode } from "@/lib/prefs";
import { SessionPanel } from "@/panels/SessionPanel";
import { FakeTransport, hostHello, tick } from "./fakes";

afterEach(cleanup);
beforeEach(() => window.localStorage.clear());

/** Mod+Shift+Enter on any platform. */
const OTHER_SEND = { key: "Enter", shiftKey: true, metaKey: true, ctrlKey: true };

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

const WORKSPACE = {
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

const summary = (follow_ups: QueuedFollowUp[]): SessionSummary => ({
  session_id: "s1",
  workspace_id: "w1",
  title: "task",
  agent: "claude_code",
  status: "in_progress",
  live: true,
  created_at: "2026-10-29T00:00:00Z",
  updated_at: "2026-10-29T00:00:00Z",
  ...(follow_ups.length > 0 ? { follow_ups } : {}),
});

const queued = (follow_up_id: string, text: string, attachments = 0): QueuedFollowUp => ({ follow_up_id, text, attachments });

/** A Host keeping session s1's follow-ups as `queue`. */
function host(queue: QueuedFollowUp[] = []) {
  const t = new FakeTransport();
  t.handlers["workspace.list"] = () => [WORKSPACE];
  t.handlers["session.list"] = () => [summary(queue)];
  t.handlers["session.follow_up"] = () => ({ outcome: "queued" });
  t.handlers["session.follow_up_edit"] = () => ({});
  t.handlers["session.follow_up_remove"] = () => ({});
  t.handlers["session.follow_up_send"] = () => ({ outcome: "queued" });
  t.handlers["session.send"] = (p) => ({ session_id: p.session_id });
  return t;
}

async function open(t: FakeTransport) {
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
  const status = (s: SessionSnapshot["status"]) =>
    act(async () =>
      t.frame({ subscription_id: "sub-1", session_id: "s1", type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot({ status: s }) } as never),
    );
  return { controller, status };
}

const calls = (t: FakeTransport, op: string) => t.calls.filter((c) => c.op === op).map((c) => c.params);

describe("follow-ups", () => {
  it("queues a message sent while the agent works, beside the stop button", async () => {
    const t = host();
    const { controller, status } = await open(t);
    await status("prompting");
    expect(screen.getByLabelText("Stop agent")).toBeTruthy();
    const box = screen.getByPlaceholderText(/^Add a follow-up/);
    fireEvent.change(box, { target: { value: "then add tests" } });
    fireEvent.click(screen.getByLabelText("Queue message"));
    await tick();
    const [sent] = calls(t, "session.follow_up");
    expect(sent.blocks).toEqual([{ type: "text", text: "then add tests" }]);
    expect(sent.client_message_id).toMatch(/^[0-9a-f-]{36}$/);
    expect(calls(t, "session.send")).toEqual([]);
    expect((box as HTMLTextAreaElement).value).toBe("");
    controller.dispose();
  });

  it("goes through the host's queue when idle too, so it never jumps the queue", async () => {
    const t = host();
    t.handlers["session.follow_up"] = () => ({ outcome: "sent" });
    const { controller, status } = await open(t);
    await status("connected");
    const box = screen.getByPlaceholderText(/^Send a message/);
    fireEvent.change(box, { target: { value: "hello" } });
    fireEvent.keyDown(box, { key: "Enter" });
    await tick();
    expect(calls(t, "session.follow_up")).toHaveLength(1);
    expect(calls(t, "session.send")).toEqual([]);
    controller.dispose();
  });

  it("lists the queue to edit, remove, or send one next", async () => {
    const t = host([queued("f1", "first"), queued("f2", "", 2)]);
    const { controller, status } = await open(t);
    await status("prompting");
    const list = within(screen.getByTestId("follow-ups"));
    expect(list.getByRole("status").textContent).toBe("Queued");
    const [first, second] = list.getAllByTestId("follow-up");
    expect(first!.textContent).toContain("first");
    expect(second!.textContent).toContain("Attachments only");
    expect(second!.textContent).toContain("2");
    // The first already goes next.
    expect(within(first!).queryByText("Send next")).toBeNull();

    fireEvent.click(within(second!).getByText("Send next"));
    await tick();
    expect(calls(t, "session.follow_up_send")).toEqual([{ session_id: "s1", follow_up_id: "f2" }]);

    fireEvent.click(within(first!).getByLabelText("Edit follow-up"));
    const input = list.getByLabelText("Follow-up text");
    fireEvent.change(input, { target: { value: "  first, but better " } });
    fireEvent.keyDown(input, { key: "Enter" });
    await tick();
    expect(calls(t, "session.follow_up_edit")).toEqual([{ session_id: "s1", follow_up_id: "f1", text: "first, but better" }]);
    expect(list.queryByLabelText("Follow-up text")).toBeNull();

    fireEvent.click(within(first!).getByLabelText("Remove follow-up"));
    await tick();
    expect(calls(t, "session.follow_up_remove")).toEqual([{ session_id: "s1", follow_up_id: "f1" }]);
    controller.dispose();
  });

  it("shows a held queue as paused, each one sendable now", async () => {
    const t = host([queued("f1", "first"), queued("f2", "second")]);
    const { controller, status } = await open(t);
    await status("connected");
    const list = within(screen.getByTestId("follow-ups"));
    expect(list.getByRole("status").textContent).toMatch(/^Paused/);
    expect(list.getAllByText("Send now")).toHaveLength(2);
    // A new message joins the held queue rather than going ahead of it.
    expect(screen.getByLabelText("Queue message")).toBeTruthy();
    fireEvent.click(list.getAllByText("Send now")[0]!);
    await tick();
    expect(calls(t, "session.follow_up_send")).toEqual([{ session_id: "s1", follow_up_id: "f1" }]);
    controller.dispose();
  });

  it("follows the queue as the host changes it", async () => {
    const t = host([queued("f1", "first")]);
    const { controller, status } = await open(t);
    await status("prompting");
    expect(screen.getAllByTestId("follow-up")).toHaveLength(1);
    await t.feedEvent("session.changed", { session: summary([]) });
    expect(screen.queryByTestId("follow-ups")).toBeNull();
    controller.dispose();
  });

  it("only drafts while the agent works with a host that keeps no queue", async () => {
    const t = host([queued("f1", "first")]);
    t.handlers["host.hello"] = () => hostHello({ capabilities: CLIENT_CAPABILITIES.filter((x) => x !== "session.follow_ups") });
    const { controller, status } = await open(t);
    await status("prompting");
    expect(screen.getByPlaceholderText(/^Draft your next message/)).toBeTruthy();
    expect(screen.queryByLabelText("Queue message")).toBeNull();
    expect(screen.queryByTestId("follow-ups")).toBeNull();
    controller.dispose();
  });

  it("steers the running turn when chosen, and Mod+Shift+Enter queues instead", async () => {
    rememberFollowUpMode("steer");
    const t = host();
    t.handlers["session.follow_up"] = (p) => ({ outcome: p.steer ? "steered" : "queued" });
    const { controller, status } = await open(t);
    await status("prompting");
    const box = screen.getByPlaceholderText(/^Add a follow-up/);
    fireEvent.change(box, { target: { value: "use the cache" } });
    fireEvent.click(screen.getByLabelText("Steer"));
    await tick();
    fireEvent.change(box, { target: { value: "then tests" } });
    fireEvent.keyDown(box, OTHER_SEND);
    await tick();
    const [steered, queuedOne] = calls(t, "session.follow_up");
    expect(steered.steer).toBe(true);
    expect(steered.blocks).toEqual([{ type: "text", text: "use the cache" }]);
    expect(queuedOne.steer).toBeUndefined();
    expect(screen.queryByTestId("steer-queued")).toBeNull();

    // Idle, there is no turn to steer: send just sends.
    await status("connected");
    expect(screen.getByLabelText("Send")).toBeTruthy();
    controller.dispose();
  });

  it("says when a steer had to wait, and steers on Mod+Shift+Enter by default", async () => {
    const t = host();
    const { controller, status } = await open(t);
    await status("prompting");
    expect(screen.getByLabelText("Queue message").getAttribute("title")).toMatch(/· Steer \(/);
    const box = screen.getByPlaceholderText(/^Add a follow-up/);
    fireEvent.change(box, { target: { value: "use the cache" } });
    fireEvent.keyDown(box, OTHER_SEND);
    await tick();
    expect(calls(t, "session.follow_up")[0]!.steer).toBe(true);
    expect(screen.getByTestId("steer-queued").textContent).toMatch(/waits in the queue/);
    // The turn ending clears it.
    await status("connected");
    expect(screen.queryByTestId("steer-queued")).toBeNull();
    controller.dispose();
  });

  it("only queues with a host that cannot steer", async () => {
    rememberFollowUpMode("steer");
    const t = host();
    t.handlers["host.hello"] = () => hostHello({ capabilities: CLIENT_CAPABILITIES.filter((x) => x !== "session.steer") });
    const { controller, status } = await open(t);
    await status("prompting");
    const box = screen.getByPlaceholderText(/^Add a follow-up/);
    fireEvent.change(box, { target: { value: "x" } });
    fireEvent.keyDown(box, OTHER_SEND);
    await tick();
    expect(calls(t, "session.follow_up")).toEqual([]);
    fireEvent.click(screen.getByLabelText("Queue message"));
    await tick();
    expect(calls(t, "session.follow_up")[0]!.steer).toBeUndefined();
    controller.dispose();
  });
});
