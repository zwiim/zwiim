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

describe("connection methods", () => {
  it("send each connection operation, the changing ones as mutating", async () => {
    const t = await live();
    const sent = () => t.transport.requests.at(-1);
    const connection = { connection_uid: "c1", status: "active" };

    t.transport.handlers["connection.list"] = () => ({ connections: [connection] });
    expect(await t.controller.listConnections()).toEqual({ connections: [connection] });
    expect(sent()).toMatchObject({ op: "connection.list", params: {} });

    const input = {
      name: "Linear",
      url: "https://mcp.linear.app/mcp",
      auth: { kind: "bearer" as const },
      label: "Work",
      secret: "k3y",
    };
    t.transport.handlers["connection.create"] = () => ({ connection, validation_error: null });
    expect((await t.controller.createConnection(input)).connection).toEqual(connection);
    expect(sent()).toMatchObject({ op: "connection.create", params: input, options: { mutating: true } });

    t.transport.handlers["connection.validate"] = () => ({
      connection,
      changes: { added: ["search"], changed: [], removed: [] },
    });
    expect((await t.controller.validateConnection("c1")).changes.added).toEqual(["search"]);
    expect(sent()).toMatchObject({ op: "connection.validate", params: { connection_uid: "c1" }, options: { mutating: true } });

    t.transport.handlers["connection.revoke"] = () => ({ connection: { ...connection, status: "revoked" } });
    expect((await t.controller.revokeConnection("c1")).connection.status).toBe("revoked");
    expect(sent()).toMatchObject({ op: "connection.revoke", params: { connection_uid: "c1" }, options: { mutating: true } });

    t.transport.handlers["connection.tools"] = () => ({ tools: [] });
    await t.controller.connectionTools("c1");
    expect(sent()).toMatchObject({ op: "connection.tools", params: { connection_uid: "c1" } });
    expect(sent()?.options?.mutating).toBeFalsy();
  });
});
