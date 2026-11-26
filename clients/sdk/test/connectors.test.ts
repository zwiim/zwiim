import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { HostController } from "../src/client/controller";
import { HostClient } from "../src/client/host-client";
import { FakeTransport, flush } from "./fakes";

async function live() {
  const transport = new FakeTransport();
  const client = new HostClient({ transport, random: () => 0.5 });
  const controller = new HostController(client);
  client.start();
  await flush();
  transport.channel.open();
  await flush();
  return { transport, controller };
}

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe("connector methods", () => {
  it("send each connector operation, the changing ones as mutating", async () => {
    const t = await live();
    const sent = () => t.transport.requests.at(-1);
    const connector = { connector_id: "c1", status: "active" };

    t.transport.handlers["connector.list"] = () => ({ connectors: [connector] });
    expect(await t.controller.listConnectors()).toEqual({ connectors: [connector] });
    expect(sent()).toMatchObject({ op: "connector.list", params: {} });

    const input = {
      name: "Linear",
      url: "https://mcp.linear.app/mcp",
      auth: { kind: "bearer" as const },
      label: "Work",
      secret: "k3y",
    };
    t.transport.handlers["connector.create"] = () => ({ connector, validation_error: null });
    expect((await t.controller.createConnector(input)).connector).toEqual(connector);
    expect(sent()).toMatchObject({ op: "connector.create", params: input, options: { mutating: true } });

    t.transport.handlers["connector.validate"] = () => ({
      connector,
      changes: { added: ["search"], changed: [], removed: [] },
    });
    expect((await t.controller.validateConnector("c1")).changes.added).toEqual(["search"]);
    expect(sent()).toMatchObject({ op: "connector.validate", params: { connector_id: "c1" }, options: { mutating: true } });

    t.transport.handlers["connector.set_secret"] = () => ({ connector, validation_error: null });
    await t.controller.setConnectorSecret("c1", "n3w");
    expect(sent()).toMatchObject({
      op: "connector.set_secret",
      params: { connector_id: "c1", secret: "n3w" },
      options: { mutating: true },
    });

    t.transport.handlers["connector.revoke"] = () => ({ connector: { ...connector, status: "revoked" } });
    expect((await t.controller.revokeConnector("c1")).connector.status).toBe("revoked");
    expect(sent()).toMatchObject({ op: "connector.revoke", params: { connector_id: "c1" }, options: { mutating: true } });

    const grant = { grant_id: 7, workspace_id: null, agent: null, default_policy: "ask", new_tools: "deny", tools: [] };
    t.transport.handlers["connector.grant.list"] = () => ({ grants: [grant] });
    expect((await t.controller.listConnectorGrants("c1")).grants).toEqual([grant]);
    expect(sent()).toMatchObject({ op: "connector.grant.list", params: { connector_id: "c1" } });

    t.transport.handlers["connector.grant.set"] = () => ({ grant });
    await t.controller.setConnectorGrant({
      connector_id: "c1",
      agent: "codex",
      default_policy: "ask",
      new_tools: "deny",
    });
    expect(sent()).toMatchObject({
      op: "connector.grant.set",
      params: { connector_id: "c1", agent: "codex", default_policy: "ask", new_tools: "deny" },
      options: { mutating: true },
    });

    t.transport.handlers["connector.tool_policy.set"] = () => ({ grant });
    await t.controller.setConnectorToolPolicy(7, "search", "allow");
    expect(sent()).toMatchObject({
      op: "connector.tool_policy.set",
      params: { grant_id: 7, tool: "search", policy: "allow" },
      options: { mutating: true },
    });

    t.transport.handlers["connector.grant.allow_all"] = () => ({ grant });
    await t.controller.allowAllConnectorTools(7);
    expect(sent()).toMatchObject({ op: "connector.grant.allow_all", params: { grant_id: 7 }, options: { mutating: true } });

    t.transport.handlers["connector.grant.remove"] = () => ({});
    await t.controller.removeConnectorGrant(7);
    expect(sent()).toMatchObject({ op: "connector.grant.remove", params: { grant_id: 7 }, options: { mutating: true } });

    t.transport.handlers["connector.tools"] = () => ({ tools: [] });
    await t.controller.connectorTools("c1");
    expect(sent()).toMatchObject({ op: "connector.tools", params: { connector_id: "c1" } });
    expect(sent()?.options?.mutating).toBeFalsy();

    t.transport.handlers["connector.executions"] = () => ({ executions: [], next_cursor: null });
    const page = await t.controller.listConnectorExecutions({ connector_id: "c1", cursor: "e9", limit: 20 });
    expect(page.next_cursor).toBeNull();
    expect(sent()).toMatchObject({
      op: "connector.executions",
      params: { connector_id: "c1", cursor: "e9", limit: 20 },
    });
    expect(sent()?.options?.mutating).toBeFalsy();
  });
});
