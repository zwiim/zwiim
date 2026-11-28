import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import {
  HostClient,
  HostController,
  ZwiimClientError,
  type Connector,
  type ConnectorGrant,
  type ConnectorExecution,
  type ConnectorTool,
  type WorkspaceInfo,
} from "@zwiim/sdk";
import { HostProvider } from "@/lib/host-context";
import {
  callOutcome,
  guessName,
  isLocalUrl,
  limitThatFits,
  rootOf,
  sessionCost,
  shownGrant,
  toolChoice,
  toolGroup,
} from "@/lib/connectors";
import { ConnectorsButton, ConnectorsDialog } from "@/panels/ConnectorsDialog";
import { FakeTransport, tick } from "./fakes";

afterEach(cleanup);

const workspace = (id: string, parent: string | null = null): WorkspaceInfo => ({
  workspace_id: id,
  name: id === "w1" ? "zwiim" : id,
  alias: null,
  kind: parent ? "worktree" : "root",
  parent_workspace_id: parent,
  is_git_repo: true,
  branch: "main",
  base_branch: null,
  base_commit: null,
  removed: false,
  display_path: null,
} as WorkspaceInfo);

const tool = (name: string, extra: Partial<ConnectorTool> = {}): ConnectorTool => ({
  name,
  title: null,
  description: `What ${name} does.`,
  input_schema: {},
  output_schema: null,
  annotations: null,
  enabled: true,
  estimate: 200,
  ...extra,
});

const connector = (extra: Partial<Connector> = {}): Connector => ({
  connector_id: "c1",
  name: "Linear",
  label: "Default",
  slug: "linear",
  url: "https://mcp.linear.app/mcp",
  status: "active",
  error: null,
  last_validated_at: null,
  created_at: "2026-11-27T00:00:00Z",
  credentials: "keychain",
  endpoints: [],
  tool_count: 3,
  tool_budget: 1000,
  tools_used: 600,
  enabled: true,
  ...extra,
});

const grant = (extra: Partial<ConnectorGrant> = {}): ConnectorGrant => ({
  grant_id: 1,
  workspace_id: "w1",
  agent: null,
  default_policy: "ask",
  new_tools: "ask",
  created_at: "2026-11-27T00:00:00Z",
  tools: [],
  ...extra,
});

describe("connector helpers", () => {
  it("shows Off for a tool that is off or denied, Allow only for a current allow", () => {
    const g = grant({
      tools: [
        { name: "a", policy: "allow", changed: false },
        { name: "b", policy: "allow", changed: true },
        { name: "c", policy: "deny", changed: false },
      ],
    });
    expect(toolChoice(tool("a"), g)).toBe("allow");
    expect(toolChoice(tool("b"), g)).toBe("ask");
    expect(toolChoice(tool("c"), g)).toBe("off");
    expect(toolChoice(tool("d"), g)).toBe("ask");
    expect(toolChoice(tool("a", { enabled: false }), g)).toBe("off");
    expect(toolChoice(tool("d"), null)).toBe("ask");
  });

  it("shows this workspace's grant before every workspace's, and never one for an agent", () => {
    const mine = grant({ grant_id: 1, workspace_id: "w1" });
    const all = grant({ grant_id: 2, workspace_id: null });
    const agent = grant({ grant_id: 3, workspace_id: "w1", agent: "codex" });
    expect(shownGrant([all, agent, mine], "w1")).toBe(mine);
    expect(shownGrant([all, agent], "w1")).toBe(all);
    expect(shownGrant([agent], "w1")).toBeNull();
    expect(rootOf([workspace("w1"), workspace("w2", "w1")], "w2")).toBe("w1");
  });

  it("groups by the server's hints, and finds the smallest limit that fits", () => {
    expect(toolGroup(tool("a", { annotations: { readOnlyHint: true } }))).toBe("read");
    expect(toolGroup(tool("a", { annotations: { destructiveHint: true } }))).toBe("destructive");
    expect(toolGroup(tool("a"))).toBe("write");
    expect(limitThatFits(1240, 3120)).toBe(4500);
    expect(limitThatFits(100, 200)).toBe(1000);
    expect(limitThatFits(2000, 9000)).toBeNull();
  });

  it("adds up the tools that are on across the active connectors allowed here", () => {
    const connectors = [
      connector({ connector_id: "a", tools_used: 5000 }),
      connector({ connector_id: "b", tools_used: 4000 }),
      connector({ connector_id: "c", tools_used: 3000, status: "error" }),
      connector({ connector_id: "d", tools_used: 2000 }),
      connector({ connector_id: "e", tools_used: 1000, enabled: false }),
    ];
    const grants = {
      a: [grant({ workspace_id: "w1" })],
      b: [grant({ workspace_id: null })],
      c: [grant({ workspace_id: "w1" })],
      d: [grant({ workspace_id: "other" })],
      e: [grant({ workspace_id: null })],
    };
    expect(sessionCost(connectors, grants, "w1")).toBe(9000);
  });

  it("guesses a name from the address, and knows a local server", () => {
    expect(guessName("https://mcp.linear.app/mcp")).toBe("Linear");
    expect(guessName("nonsense")).toBe("");
    expect(guessName("http://127.0.0.1:3000/mcp")).toBe("");
    expect(guessName("http://[::1]:3000/mcp")).toBe("");
    expect(guessName("http://localhost:3000/mcp")).toBe("");
    expect(isLocalUrl("http://localhost:3000/mcp")).toBe(true);
    expect(isLocalUrl("http://192.168.1.4/mcp")).toBe(true);
    expect(isLocalUrl("https://mcp.linear.app/mcp")).toBe(false);
  });
});

const execution = (id: string, extra: Partial<ConnectorExecution> = {}): ConnectorExecution => ({
  execution_id: id,
  tool: "list_issues",
  state: "succeeded",
  reason: null,
  detail: null,
  arg_keys: ["team"],
  received_at: "2026-11-27T10:00:00Z",
  dispatched_at: "2026-11-27T10:00:00Z",
  finished_at: "2026-11-27T10:00:01Z",
  upstream_ms: 465,
  overhead_ms: 2,
  ...extra,
});

describe("callOutcome", () => {
  it("says how a call ended in a few words", () => {
    expect(callOutcome(execution("e"))).toEqual({ text: "ran", tone: "ok" });
    expect(callOutcome(execution("e", { state: "awaiting_approval" }))).toEqual({ text: "waiting for you", tone: "brand" });
    expect(callOutcome(execution("e", { state: "rejected", reason: "not_exposed" })).text).toBe("tool off");
    expect(callOutcome(execution("e", { state: "rejected", reason: "denied" })).text).toBe("not allowed");
    expect(callOutcome(execution("e", { state: "approval_denied" })).text).toBe("you denied");
    expect(callOutcome(execution("e", { state: "outcome_unknown" }))).toEqual({ text: "may have run", tone: "brand" });
    expect(callOutcome(execution("e", { state: "tool_error" })).tone).toBe("bad");
  });
});

/** A Host with connectors that answers the way the real one does. */
function connectorHost(
  init: { connectors?: Connector[]; grants?: ConnectorGrant[]; tools?: ConnectorTool[] } = {},
) {
  const t = new FakeTransport();
  const state = {
    connectors: init.connectors ?? [connector()],
    grants: init.grants ?? [grant()],
    tools: init.tools ?? [
      tool("list_issues", { annotations: { readOnlyHint: true } }),
      tool("create_issue"),
      tool("delete_comment", { annotations: { destructiveHint: true }, enabled: false, estimate: 700 }),
    ],
    nextGrant: 10,
  };
  const c = () => state.connectors[0]!;
  const used = () => state.tools.filter((x) => x.enabled).reduce((n, x) => n + x.estimate, 0);
  const sync = () => {
    if (state.connectors[0]) state.connectors[0] = { ...c(), tools_used: used() };
  };
  sync();
  t.handlers["workspace.list"] = () => [workspace("w1"), workspace("w2", "w1")];
  t.handlers["connector.list"] = () => ({ connectors: state.connectors });
  t.handlers["connector.grant.list"] = () => ({ grants: state.grants });
  t.handlers["connector.tools"] = () => ({ tools: state.tools });
  t.handlers["connector.executions"] = () => ({ executions: [], next_cursor: null });
  t.handlers["connector.tool.set_enabled"] = (p: { tool: string; enabled: boolean }) => {
    const target = state.tools.find((x) => x.name === p.tool)!;
    if (p.enabled && !target.enabled && used() + target.estimate > c().tool_budget) {
      throw new ZwiimClientError("over_budget", "Over the limit", 409, {
        needs: target.estimate,
        left: c().tool_budget - used(),
        tool_budget: c().tool_budget,
      });
    }
    state.tools = state.tools.map((x) => (x.name === p.tool ? { ...x, enabled: p.enabled } : x));
    sync();
    return { connector: c(), tools: state.tools };
  };
  t.handlers["connector.set_enabled"] = (p: { enabled: boolean }) => {
    state.connectors[0] = { ...c(), enabled: p.enabled };
    return { connector: c() };
  };
  t.handlers["connector.tool_budget.set"] = (p: { tool_budget: number }) => {
    state.connectors[0] = { ...c(), tool_budget: p.tool_budget };
    return { connector: c() };
  };
  t.handlers["connector.tool_policy.set"] = (p: { grant_id: number; tool: string; policy: "allow" | "ask" | "deny" }) => {
    state.grants = state.grants.map((g) =>
      g.grant_id === p.grant_id
        ? { ...g, tools: [...g.tools.filter((r) => r.name !== p.tool), { name: p.tool, policy: p.policy, changed: false }] }
        : g,
    );
    return { grant: state.grants.find((g) => g.grant_id === p.grant_id)! };
  };
  t.handlers["connector.grant.set"] = (p: { workspace_id?: string }) => {
    const scope = p.workspace_id ?? null;
    const found = state.grants.find((g) => g.workspace_id === scope && g.agent === null);
    if (found) return { grant: found };
    const made = grant({ grant_id: state.nextGrant++, workspace_id: scope });
    state.grants = [...state.grants, made];
    return { grant: made };
  };
  t.handlers["connector.grant.remove"] = (p: { grant_id: number }) => {
    state.grants = state.grants.filter((g) => g.grant_id !== p.grant_id);
    return {};
  };
  return { t, state };
}

async function mount(t: FakeTransport, ui: "dialog" | "button" = "dialog") {
  const client = new HostClient({ transport: t });
  const controller = new HostController(client);
  client.start();
  await tick();
  render(
    <HostProvider value={controller}>
      {ui === "dialog" ? <ConnectorsDialog workspaceId="w2" onClose={() => {}} /> : <ConnectorsButton workspaceId="w2" />}
    </HostProvider>,
  );
  await tick();
}

const ops = (t: FakeTransport, op: string) => t.calls.filter((c) => c.op === op).map((c) => c.params);
const row = (name: string) => within(screen.getByTestId(`tool-${name}`));
const pick = async (name: string, choice: "Off" | "Ask" | "Allow") => {
  fireEvent.click(row(name).getByRole("radio", { name: choice }));
  await tick();
};
const checked = (name: string) => row(name).getByRole("radio", { checked: true }).textContent;

describe("ConnectorsDialog", () => {
  it("lists the connector under the worktree's root and shows its tools by group", async () => {
    const { t } = connectorHost();
    await mount(t);
    expect(screen.getByText("Allowed in zwiim · 1")).toBeTruthy();
    expect(screen.getByRole("radio", { name: "zwiim", checked: true })).toBeTruthy();
    expect(checked("list_issues")).toBe("Ask");
    expect(checked("delete_comment")).toBe("Off");
    expect(screen.getByText("Read · 1")).toBeTruthy();
    expect(screen.getByText("Destructive · 1")).toBeTruthy();
    expect(screen.getByTestId("connector-c1").textContent).toContain("400 / 1,000");
  });

  it("Off turns the tool's switch off; Allow sets this workspace's row", async () => {
    const { t } = connectorHost();
    await mount(t);
    await pick("create_issue", "Off");
    expect(ops(t, "connector.tool.set_enabled")).toEqual([{ connector_id: "c1", tool: "create_issue", enabled: false }]);
    expect(checked("create_issue")).toBe("Off");

    await pick("list_issues", "Allow");
    expect(ops(t, "connector.tool_policy.set")).toEqual([{ grant_id: 1, tool: "list_issues", policy: "allow" }]);
    expect(checked("list_issues")).toBe("Allow");
  });

  it("turns a whole connector off and on, keeping its settings", async () => {
    const { t } = connectorHost();
    await mount(t);
    const enabled = () => screen.getByRole("switch", { name: "Enabled" });
    expect(enabled().getAttribute("aria-checked")).toBe("true");

    fireEvent.click(enabled());
    await tick();
    expect(ops(t, "connector.set_enabled")).toEqual([{ connector_id: "c1", enabled: false }]);
    expect(enabled().getAttribute("aria-checked")).toBe("false");
    expect(screen.getByText("Agents get none of its tools. Its key and settings are kept")).toBeTruthy();
    expect(screen.getAllByText("turned off").length).toBeGreaterThan(0);
    expect(checked("list_issues")).not.toBe("Off");

    fireEvent.click(enabled());
    await tick();
    expect(ops(t, "connector.set_enabled")).toEqual([
      { connector_id: "c1", enabled: false },
      { connector_id: "c1", enabled: true },
    ]);
    expect(enabled().getAttribute("aria-checked")).toBe("true");
  });

  it("a tool that doesn't fit offers a higher limit, then turns on", async () => {
    const { t } = connectorHost();
    await mount(t);
    await pick("delete_comment", "Ask");
    const prompt = row("delete_comment").getByRole("alert");
    expect(prompt.textContent).toContain("delete_comment needs ~700.");
    expect(prompt.textContent).toContain("600 left of 1,000.");
    expect(checked("delete_comment")).toBe("Off");

    fireEvent.click(within(prompt).getByRole("button", { name: "Raise limit to 1,500" }));
    await tick();
    expect(ops(t, "connector.tool_budget.set")).toEqual([{ connector_id: "c1", tool_budget: 1500 }]);
    expect(checked("delete_comment")).toBe("Ask");
    expect(row("delete_comment").queryByRole("alert")).toBeNull();
    const meter = screen.getByRole("meter", { name: "Tool limit used" });
    expect([meter.getAttribute("aria-valuenow"), meter.getAttribute("aria-valuemax")]).toEqual(["1100", "1500"]);
  });

  it("moving to every workspace keeps each tool's row", async () => {
    const { t, state } = connectorHost({
      grants: [grant({ tools: [{ name: "list_issues", policy: "allow", changed: false }] })],
    });
    await mount(t);
    fireEvent.click(screen.getByRole("radio", { name: "All workspaces" }));
    await tick();
    expect(state.grants).toHaveLength(1);
    expect(state.grants[0]!.workspace_id).toBeNull();
    expect(state.grants[0]!.tools).toEqual([{ name: "list_issues", policy: "allow", changed: false }]);
    expect(screen.getByRole("radio", { name: "All workspaces", checked: true })).toBeTruthy();
    expect(checked("list_issues")).toBe("Allow");

    fireEvent.click(screen.getByRole("radio", { name: "Nowhere" }));
    await tick();
    expect(state.grants).toEqual([]);
    expect(screen.getByText("Not allowed here · 1")).toBeTruthy();
    expect((row("list_issues").getByRole("radio", { name: "Allow" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("Allow all read allows only read tools that are on", async () => {
    const { t } = connectorHost({
      tools: [
        tool("list_issues", { annotations: { readOnlyHint: true } }),
        tool("get_issue", { annotations: { readOnlyHint: true }, enabled: false }),
        tool("create_issue"),
      ],
    });
    await mount(t);
    fireEvent.click(screen.getByRole("button", { name: "Allow all read" }));
    await tick();
    expect(ops(t, "connector.tool_policy.set")).toEqual([{ grant_id: 1, tool: "list_issues", policy: "allow" }]);
  });

  it("has no Allow all read when no tool says it only reads", async () => {
    const { t } = connectorHost({ tools: [tool("ask_question"), tool("read_contents")] });
    await mount(t);
    expect(screen.queryByRole("button", { name: "Allow all read" })).toBeNull();
  });

  it("Activity lists the calls, adds new ones as they come, and opens one", async () => {
    const { t } = connectorHost();
    t.handlers["connector.executions"] = () => ({
      executions: [execution("e1", { state: "approval_denied", detail: "The user denied this call; nothing was sent.", upstream_ms: null })],
      next_cursor: null,
    });
    await mount(t);
    fireEvent.click(screen.getByRole("button", { name: /Activity/ }));
    await tick();
    expect(ops(t, "connector.executions").at(-1)).toEqual({ connector_id: "c1", limit: 50 });
    const first = within(screen.getByTestId("call-e1"));
    expect(first.getByText("you denied")).toBeTruthy();

    act(() => {
      t.feedEvent("connector.execution", { connector_id: "c1", execution: execution("e2", { tool: "create_issue", received_at: "2026-11-27T10:05:00Z" }) });
      t.feedEvent("connector.execution", { connector_id: "other", execution: execution("e3") });
    });
    await tick();
    const calls = screen.getAllByTestId(/^call-/).map((e) => e.dataset.testid);
    expect(calls).toEqual(["call-e2", "call-e1"]);
    expect(within(screen.getByTestId("call-e2")).getByText("465 ms")).toBeTruthy();

    fireEvent.click(first.getByRole("button", { name: /list_issues/ }));
    expect(screen.getByText("The user denied this call; nothing was sent.")).toBeTruthy();
    expect(screen.getByText("team")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    expect(screen.getByTestId("tool-list_issues")).toBeTruthy();
  });

  it("warns when the connectors allowed here take more than 12,000 together", async () => {
    const { t } = connectorHost();
    await mount(t);
    expect(screen.queryByRole("note")).toBeNull();
    cleanup();

    const big = connectorHost({
      connectors: [
        connector(),
        connector({ connector_id: "c2", name: "GitHub", slug: "github", tool_budget: 10000, tools_used: 6000 }),
        connector({ connector_id: "c3", name: "Notion", slug: "notion", tool_budget: 10000, tools_used: 6000 }),
      ],
    });
    await mount(big.t);
    expect(screen.getByRole("note").textContent).toContain("about 12,400 of an agent's context, over 12,000");
  });

  it("asks for a new key when the server refused the one it has", async () => {
    const { t } = connectorHost({
      connectors: [connector({ status: "needs_reauth", error: "the service refused the key" })],
    });
    await mount(t);
    expect(screen.getByText("Linear refused the key. Paste a new one below.")).toBeTruthy();
    expect(screen.getByLabelText("New key")).toBeTruthy();
    expect(screen.getByTestId("connector-c1").textContent).toContain("needs a key");
  });

  it("search narrows the tools; a name opens its description", async () => {
    const { t } = connectorHost();
    await mount(t);
    fireEvent.change(screen.getByLabelText("Search tools"), { target: { value: "issue" } });
    expect(screen.queryByTestId("tool-delete_comment")).toBeNull();
    fireEvent.click(row("create_issue").getByRole("button", { name: "create_issue" }));
    expect(screen.getByText("What create_issue does.")).toBeTruthy();
  });

  it("adds a connector and allows it in this workspace", async () => {
    const { t, state } = connectorHost({ connectors: [], grants: [] });
    t.handlers["connector.create"] = (p) => {
      const made = connector({ connector_id: "c2", name: p.name, url: p.url });
      state.connectors = [made];
      return { connector: made, validation_error: null };
    };
    await mount(t);
    expect(screen.getByText("Add a connector")).toBeTruthy();
    fireEvent.change(screen.getByLabelText("Server URL"), { target: { value: "https://mcp.figma.com/mcp" } });
    fireEvent.change(screen.getByLabelText("API key"), { target: { value: "figd_secret" } });
    fireEvent.click(screen.getByRole("button", { name: "Add Figma" }));
    await tick();
    expect(ops(t, "connector.create")).toEqual([
      {
        name: "Figma",
        url: "https://mcp.figma.com/mcp",
        label: "Default",
        network: "public",
        auth: { kind: "bearer" },
        secret: "figd_secret",
      },
    ]);
    expect(ops(t, "connector.grant.set")).toEqual([
      { connector_id: "c2", workspace_id: "w1", default_policy: "ask", new_tools: "ask" },
    ]);
    expect(screen.getByTestId("connector-c2").getAttribute("aria-current")).toBe("true");
  });

  it("the composer's plug counts the connectors agents here can use", async () => {
    const { t } = connectorHost();
    await mount(t, "button");
    expect(screen.getByRole("button", { name: "Connectors, 1 on here" })).toBeTruthy();
  });

  it("the plug is hidden for a Host from before connectors", async () => {
    const t = new FakeTransport();
    t.handlers["connector.list"] = () => {
      throw new ZwiimClientError("unknown_operation", "unknown op", 404);
    };
    await mount(t, "button");
    await act(async () => {});
    expect(screen.queryByRole("button", { name: /Connectors/ })).toBeNull();
  });
});
