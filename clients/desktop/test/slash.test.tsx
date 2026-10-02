import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { HostClient, HostController, type SessionCommand, type SessionSnapshot } from "@zwiim/sdk";
import { HostProvider } from "@/lib/host-context";
import { SessionPanel } from "@/panels/SessionPanel";
import { matchCommands, slashQuery } from "@/panels/SlashCommands";
import { FakeTransport, tick } from "./fakes";

afterEach(cleanup);

const commands: SessionCommand[] = [
  { name: "review", description: "Review a pull request", hint: "pr number" },
  { name: "compact", description: "Compact the conversation", hint: null },
  { name: "pr-comments", description: "Fetch PR comments", hint: null },
];

const snapshot = (commands: SessionCommand[]): SessionSnapshot => ({
  status: "connected",
  live_message: null,
  tool_calls: [],
  pending_request: null,
  pending_user_message: null,
  last_error: null,
  usage: null,
  options: null,
  commands,
});

describe("slash query", () => {
  it("finds the /word being typed at the start or after a space", () => {
    expect(slashQuery("/re", 3)).toEqual({ start: 0, query: "re" });
    expect(slashQuery("please /", 8)).toEqual({ start: 7, query: "" });
    expect(slashQuery("a/b", 3)).toBeNull();
    expect(slashQuery("/review 12", 10)).toBeNull();
  });

  it("lists prefix matches before other matches", () => {
    expect(matchCommands(commands, "r").map((c) => c.name)).toEqual(["review", "pr-comments"]);
  });
});

describe("slash menu", () => {
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
    const frame = (f: object) =>
      act(async () => t.frame({ subscription_id: "sub-1", session_id: "s1", ...f } as never));
    return { controller, frame };
  }

  it("offers the session's commands and writes the picked one into the message", async () => {
    const t = new FakeTransport();
    t.handlers["session.send"] = (p) => ({ session_id: p.session_id });
    const { controller, frame } = await open(t);
    await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot(commands) });
    const box = screen.getByPlaceholderText("Send a message") as HTMLTextAreaElement;

    fireEvent.change(box, { target: { value: "/" } });
    expect(screen.getAllByRole("option")).toHaveLength(3);
    fireEvent.change(box, { target: { value: "/r" } });
    expect(screen.getAllByRole("option").map((o) => o.textContent)).toEqual([
      expect.stringContaining("/review"),
      expect.stringContaining("/pr-comments"),
    ]);
    fireEvent.keyDown(box, { key: "ArrowDown" });
    expect(screen.getAllByRole("option")[1]!.getAttribute("aria-selected")).toBe("true");
    fireEvent.keyDown(box, { key: "ArrowUp" });
    // Enter picks from the menu; it does not send.
    fireEvent.keyDown(box, { key: "Enter" });
    expect(box.value).toBe("/review ");
    expect(screen.queryByRole("listbox", { name: "Commands" })).toBeNull();
    expect(t.calls.some((c) => c.op === "session.send")).toBe(false);

    fireEvent.change(box, { target: { value: "/review 12" } });
    fireEvent.keyDown(box, { key: "Enter" });
    await tick();
    expect(t.calls.find((c) => c.op === "session.send")!.params.blocks).toEqual([{ type: "text", text: "/review 12" }]);
    controller.dispose();
  });

  it("closes with Escape and follows later command updates", async () => {
    const t = new FakeTransport();
    const { controller, frame } = await open(t);
    await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot([]) });
    const box = screen.getByPlaceholderText("Send a message");
    fireEvent.change(box, { target: { value: "/" } });
    expect(screen.queryByRole("listbox")).toBeNull();

    await frame({
      type: "event",
      event: { session_id: "s1", seq: 2, type: "session.commands", payload: { commands } },
    });
    expect(screen.getByRole("listbox", { name: "Commands" })).toBeTruthy();
    fireEvent.keyDown(box, { key: "Escape" });
    expect(screen.queryByRole("listbox")).toBeNull();
    // Typing on reopens nothing for the dismissed `/`; a new one does.
    fireEvent.change(box, { target: { value: "/c" } });
    expect(screen.queryByRole("listbox")).toBeNull();
    fireEvent.change(box, { target: { value: "/c /" } });
    expect(screen.getByRole("listbox", { name: "Commands" })).toBeTruthy();
    controller.dispose();
  });

  it("offers a new session the agent's commands from its last session", async () => {
    const t = new FakeTransport();
    t.handlers["agent.list"] = () => [
      { agent: "claude_code", name: "Claude Code", available: true, reason: null, support: "supported", options: null, commands },
    ];
    const client = new HostClient({ transport: t });
    const controller = new HostController(client);
    client.start();
    await tick();
    render(
      <HostProvider value={controller}>
        <SessionPanel workspaceId="w1" sessionId={null} onSessionStarted={() => {}} />
      </HostProvider>,
    );
    await tick();
    fireEvent.change(screen.getByPlaceholderText("Describe the task…"), { target: { value: "/comp" } });
    const menu = screen.getByRole("listbox", { name: "Commands" });
    expect(
      within(menu)
        .getAllByRole("option")
        .map((o) => o.textContent),
    ).toEqual([expect.stringContaining("/compact")]);
    controller.dispose();
  });
});
