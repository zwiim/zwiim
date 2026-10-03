import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { CLIENT_CAPABILITIES, HostClient, HostController, type SessionCommand, type SessionSnapshot } from "@zwiim/sdk";
import { HostProvider } from "@/lib/host-context";
import { SessionPanel } from "@/panels/SessionPanel";
import { matchCommands, slashQuery, terminalCommand } from "@/panels/SlashCommands";
import { AgentTerminalsContext, useAgentTerminalsState } from "@/panels/AgentTerminal";
import { TerminalPanel } from "@/panels/TerminalPanel";
import { FakeTransport, hostHello, tick } from "./fakes";

vi.mock("@xterm/xterm", () => ({
  Terminal: class {
    loadAddon() {}
    open() {}
    focus() {}
    reset() {}
    write() {}
    dispose() {}
    onData() {
      return { dispose() {} };
    }
    onResize() {
      return { dispose() {} };
    }
  },
}));
vi.mock("@xterm/addon-fit", () => ({
  FitAddon: class {
    fit() {}
  },
}));
globalThis.ResizeObserver ??= class {
  observe() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

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

  it("offers the agent's CLI a /name opening the message that the agent does not list", () => {
    expect(terminalCommand(slashQuery("/mcp", 4), commands)).toBe("/mcp");
    expect(terminalCommand(slashQuery("/review", 7), commands)).toBeNull();
    expect(terminalCommand(slashQuery("/rev", 4), commands)).toBeNull();
    expect(terminalCommand(slashQuery("/review", 7), null)).toBe("/review");
    expect(terminalCommand(slashQuery("/", 1), commands)).toBeNull();
    expect(terminalCommand(slashQuery("see /mcp", 8), commands)).toBeNull();
    expect(terminalCommand(slashQuery("/src/app", 8), commands)).toBeNull();
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
    const box = screen.getByPlaceholderText(/^Send a message/) as HTMLTextAreaElement;
    expect(box.placeholder).toBe("Send a message, or type / for commands");

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
    const box = screen.getByPlaceholderText(/^Send a message/) as HTMLTextAreaElement;
    // Nothing to offer yet, so no word of commands.
    expect(box.placeholder).toBe("Send a message");
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
    fireEvent.change(screen.getByPlaceholderText(/^Describe the task/), { target: { value: "/comp" } });
    const menu = screen.getByRole("listbox", { name: "Commands" });
    expect(
      within(menu)
        .getAllByRole("option")
        .map((o) => o.textContent),
    ).toEqual([expect.stringContaining("/compact")]);
    controller.dispose();
  });
});

describe("slash commands in the embedded terminal", () => {
  const managed = {
    version: "1",
    locked_version: "1",
    installed: true,
    healthy: true,
    authenticated: true,
    usable: true,
    setup: { state: "ready" },
    source: "managed",
  };

  /** The session and the workspace's terminal dock, as the app lays them out. */
  function Workbench() {
    const terminals = useAgentTerminalsState();
    return (
      <AgentTerminalsContext.Provider value={terminals}>
        <SessionPanel workspaceId="w1" sessionId={null} onSessionStarted={() => {}} />
        <TerminalPanel workspaceId="w1" hidden={terminals.ids} />
      </AgentTerminalsContext.Provider>
    );
  }

  async function open(t: FakeTransport, listed = commands) {
    t.handlers["agent.list"] = () => [
      {
        agent: "codex",
        name: "Codex",
        available: true,
        reason: null,
        support: "supported",
        adapter: managed,
        options: null,
        effort_by_model: {},
        commands: listed,
      },
    ];
    let spawned = 0;
    t.handlers["terminal.spawn"] = () => ({ terminal_id: `t${++spawned}` });
    t.handlers["terminal.snapshot"] = (p) => ({ terminal_id: p.terminal_id, alive: true, data: "", seq: 0 });
    t.handlers["terminal.kill"] = () => ({});
    const client = new HostClient({ transport: t });
    const controller = new HostController(client);
    client.start();
    await tick();
    render(
      <HostProvider value={controller}>
        <Workbench />
      </HostProvider>,
    );
    await tick();
    const box = screen.getByPlaceholderText(/^Describe the task/) as HTMLTextAreaElement;
    expect(box.placeholder).toBe("Describe the task, or type / for commands");
    return { controller, box };
  }

  async function run(box: HTMLTextAreaElement, command: string) {
    fireEvent.change(box, { target: { value: command } });
    fireEvent.keyDown(box, { key: "Enter" });
    await tick();
  }

  const kills = (t: FakeTransport) => t.calls.filter((c) => c.op === "terminal.kill").map((c) => c.params);

  it("runs a command in the agent's CLI over the composer, until Done", async () => {
    const t = new FakeTransport();
    const { controller, box } = await open(t);
    fireEvent.change(box, { target: { value: "/mcp" } });
    const row = within(screen.getByRole("listbox", { name: "Commands" })).getByRole("option");
    expect(row.textContent).toContain("Run /mcp in the embedded terminal");
    expect(row.textContent).toContain("Codex");

    fireEvent.keyDown(box, { key: "Enter" });
    await tick();
    expect(t.calls.find((c) => c.op === "terminal.spawn")!.params).toEqual({
      workspace_id: "w1",
      agent: "codex",
      command: "/mcp",
    });
    const card = screen.getByRole("region", { name: "Running /mcp" });
    expect(card.textContent).toContain("Complete /mcp below, then click Done");
    expect(within(card).getByTestId("terminal")).toBeTruthy();
    // It runs there, not in the dock, and leaves the message; nothing was sent.
    expect(screen.getByText("No terminals in this workspace.")).toBeTruthy();
    expect(box.value).toBe("");
    expect(t.calls.some((c) => c.op === "session.start")).toBe(false);

    fireEvent.click(within(card).getByRole("button", { name: "Done" }));
    await tick();
    expect(kills(t)).toEqual([{ terminal_id: "t1" }]);
    expect(screen.queryByRole("region", { name: "Running /mcp" })).toBeNull();
    expect(screen.getByText("No terminals in this workspace.")).toBeTruthy();
    controller.dispose();
  });

  it("closes a finished command without killing anything", async () => {
    const t = new FakeTransport();
    const { controller, box } = await open(t);
    await run(box, "/status");
    await t.feedEvent("terminal.removed", { terminal_id: "t1" });
    await tick();
    const card = screen.getByRole("region", { name: "Running /status" });
    expect(card.textContent).toContain("/status has finished");
    fireEvent.click(within(card).getByRole("button", { name: "Done" }));
    await tick();
    expect(kills(t)).toEqual([]);
    expect(screen.queryByRole("region")).toBeNull();
    controller.dispose();
  });

  it("replaces the workspace's last command with a new one", async () => {
    const t = new FakeTransport();
    const { controller, box } = await open(t);
    await run(box, "/mcp");
    await run(box, "/status");
    expect(screen.queryByRole("region", { name: "Running /mcp" })).toBeNull();
    expect(screen.getByRole("region", { name: "Running /status" })).toBeTruthy();
    expect(kills(t)).toEqual([{ terminal_id: "t1" }]);
    controller.dispose();
  });

  it("gives a replaced command's terminal to the dock when it cannot be ended", async () => {
    const t = new FakeTransport();
    const { controller, box } = await open(t);
    t.handlers["terminal.kill"] = () => {
      throw new Error("offline");
    };
    await run(box, "/mcp");
    expect(screen.queryByRole("tab")).toBeNull();
    await run(box, "/status");
    expect(screen.getByRole("region", { name: "Running /status" })).toBeTruthy();
    expect(screen.getAllByRole("tab")).toHaveLength(1);
    controller.dispose();
  });

  it("offers only what the agent does not list, after its matches", async () => {
    const t = new FakeTransport();
    const { controller, box } = await open(t);
    fireEvent.change(box, { target: { value: "/pr-c" } });
    const menu = () => within(screen.getByRole("listbox", { name: "Commands" }));
    expect(menu().getAllByRole("option").map((o) => o.textContent)).toEqual([expect.stringContaining("/pr-comments")]);
    fireEvent.change(box, { target: { value: "/pr-x" } });
    expect(menu().getAllByRole("option").map((o) => o.textContent)).toEqual([
      expect.stringContaining("Run /pr-x in the embedded terminal"),
    ]);
    // One the agent lists runs in the session, from the message.
    fireEvent.change(box, { target: { value: "/review" } });
    expect(menu().getAllByRole("option").map((o) => o.textContent)).toEqual([expect.stringContaining("/review")]);
    fireEvent.keyDown(box, { key: "Enter" });
    expect(box.value).toBe("/review ");
    expect(t.calls.some((c) => c.op === "terminal.spawn")).toBe(false);
    controller.dispose();
  });

  it("runs a listed command whose screen is the CLI's from its own row", async () => {
    const t = new FakeTransport();
    const { controller, box } = await open(t, [
      ...commands,
      { name: "mcp", description: "List MCP servers", hint: null },
    ]);
    fireEvent.change(box, { target: { value: "/mc" } });
    const row = within(screen.getByRole("listbox", { name: "Commands" })).getByRole("option");
    expect(row.textContent).toContain("/mcpList MCP serversEmbedded terminal");
    fireEvent.keyDown(box, { key: "Enter" });
    await tick();
    expect(t.calls.find((c) => c.op === "terminal.spawn")!.params).toEqual({
      workspace_id: "w1",
      agent: "codex",
      command: "/mcp",
    });
    expect(screen.getByRole("region", { name: "Running /mcp" })).toBeTruthy();
    expect(box.value).toBe("");
    controller.dispose();
  });

  it("says why a terminal did not open", async () => {
    const t = new FakeTransport();
    const { controller, box } = await open(t);
    t.handlers["terminal.spawn"] = () => {
      throw new Error("no terminal for you");
    };
    await run(box, "/mcp");
    expect(screen.getByRole("alert").textContent).toContain("no terminal for you");
    expect(screen.queryByRole("region")).toBeNull();
    controller.dispose();
  });

  it("is not offered by a host without agent terminals", async () => {
    const t = new FakeTransport();
    t.handlers["host.hello"] = () =>
      hostHello({ capabilities: CLIENT_CAPABILITIES.filter((c) => c !== "terminal.agent") });
    const { controller, box } = await open(t, [{ name: "status", description: "Show status", hint: null }]);
    fireEvent.change(box, { target: { value: "/mcp" } });
    expect(screen.queryByRole("listbox")).toBeNull();
    // A listed one stays in the message.
    fireEvent.change(box, { target: { value: "/status" } });
    const row = within(screen.getByRole("listbox", { name: "Commands" })).getByRole("option");
    expect(row.textContent).not.toContain("Embedded terminal");
    fireEvent.keyDown(box, { key: "Enter" });
    expect(box.value).toBe("/status ");
    controller.dispose();
  });
});
