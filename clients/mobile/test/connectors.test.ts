import { describe, expect, it } from "vitest";
import { ZwiimClientError, type Connector } from "@zwiim/sdk";
import { connectorRow, loadConnectors, type ConnectorsHost } from "../src/host/connectors";

const connector = (extra: Partial<Connector> = {}): Connector => ({
  connector_id: "c1",
  name: "Linear",
  label: "Default",
  slug: "linear",
  url: "https://mcp.linear.app/mcp",
  status: "active",
  error: null,
  last_validated_at: null,
  created_at: "2026-11-28T00:00:00Z",
  credentials: "keychain",
  endpoints: [],
  tool_count: 3,
  tool_budget: 4000,
  tools_used: 600,
  ...extra,
});

function fakeHost(answer: () => unknown) {
  const calls: { op: string; params: unknown }[] = [];
  const host: ConnectorsHost = {
    request: (async (op: string, params: unknown) => {
      calls.push({ op, params });
      return answer();
    }) as ConnectorsHost["request"],
  };
  return { host, calls };
}

describe("connectors on the phone", () => {
  it("lists the host's connectors, leaving out revoked ones", async () => {
    const live = connector();
    const ok = fakeHost(() => ({ connectors: [live, connector({ connector_id: "c2", status: "revoked" })] }));
    expect(await loadConnectors(ok.host)).toEqual({ connectors: [live] });
    expect(ok.calls).toEqual([{ op: "connector.list", params: {} }]);
  });

  it("knows a host from before connectors, and says why a list failed", async () => {
    const old = fakeHost(() => {
      throw new ZwiimClientError("unknown_operation", "unknown op", 404);
    });
    expect(await loadConnectors(old.host)).toBe("outdated");
    const down = fakeHost(() => {
      throw new ZwiimClientError("network", "Host unreachable");
    });
    expect(await loadConnectors(down.host)).toEqual({ error: "Host unreachable" });
  });

  it("shows each one's name, server and status", () => {
    expect(connectorRow(connector())).toEqual({
      name: "Linear",
      detail: "mcp.linear.app",
      state: "Connected",
      dot: "done",
    });
    expect(connectorRow(connector({ label: "Work", status: "needs_reauth" }))).toMatchObject({
      name: "Linear (Work)",
      state: "Needs a key",
      dot: "need",
    });
    expect(connectorRow(connector({ status: "error", url: "not a url" }))).toMatchObject({
      detail: "not a url",
      state: "Can't reach server",
      dot: "err",
    });
  });
});
