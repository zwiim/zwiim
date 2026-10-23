import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  CLIENT_CAPABILITIES,
  CLIENT_REVISION,
  evaluateCompatibility,
  hasCapability,
  helloClient,
  isHostOutdated,
  type ClientCompat,
} from "../src/client/compat";
import { ZwiimClientError } from "../src/client/errors";
import { HostClient } from "../src/client/host-client";
import { initialHostState, reduceHost } from "../src/state/store";
import { initialSessionView, reduceSession } from "../src/state/session";
import type { HostFeedEvent, SessionEvent } from "../src/types/protocol";
import { FakeTransport, flush, hello } from "./fakes";

const client = (over: Partial<ClientCompat> = {}): ClientCompat => ({
  major: 1,
  revision: 3,
  minHostRevision: 2,
  capabilities: ["session.usage", "terminal"],
  ...over,
});

describe("evaluateCompatibility", () => {
  it("is compatible across different product versions", () => {
    const c = evaluateCompatibility(
      hello({ version: "9.4.0", revision: 3, min_client_revision: 1, capabilities: ["session.usage", "terminal"] }),
      client(),
    );
    expect(c).toEqual({ status: "compatible", hostRevision: 3, capabilities: ["session.usage", "terminal"] });
  });

  it("old client, new Host: Host requires a newer client revision", () => {
    const c = evaluateCompatibility(hello({ revision: 7, min_client_revision: 5 }), client({ revision: 3 }));
    expect(c).toMatchObject({
      status: "update_required",
      update: "client",
      reason: "client_revision",
      requiredClientRevision: 5,
      requiredHostRevision: null,
    });
  });

  it("new client, old Host: Host below the client's minimum", () => {
    const c = evaluateCompatibility(hello({ revision: 1, min_client_revision: 1 }), client({ minHostRevision: 2 }));
    expect(c).toMatchObject({
      status: "update_required",
      update: "host",
      reason: "host_revision",
      requiredHostRevision: 2,
      requiredClientRevision: null,
    });
  });

  it("new client, Host older than the handshake reads as revision 0", () => {
    const legacy = { host_id: "h", protocol: "zwiim/1", version: "0.1.0", owner: null };
    expect(evaluateCompatibility(legacy, client())).toMatchObject({
      status: "update_required",
      update: "host",
      hostRevision: 0,
    });
  });

  it("honours the minimum revision boundary exactly", () => {
    const at = evaluateCompatibility(hello({ revision: 3, min_client_revision: 3, capabilities: [] }), client({ revision: 3, capabilities: [] }));
    expect(at.status).toBe("compatible");
    const below = evaluateCompatibility(hello({ revision: 4, min_client_revision: 4 }), client({ revision: 3 }));
    expect(below.status).toBe("update_required");
    const hostAt = evaluateCompatibility(hello({ revision: 2, min_client_revision: 1, capabilities: [] }), client({ minHostRevision: 2, capabilities: [] }));
    expect(hostAt.status).toBe("compatible");
  });

  it("a missing optional capability is limited, not broken", () => {
    const c = evaluateCompatibility(hello({ revision: 3, min_client_revision: 1, capabilities: ["terminal"] }), client());
    expect(c).toEqual({ status: "limited", hostRevision: 3, capabilities: ["terminal"], missing: ["session.usage"] });
    expect(hasCapability(c, "terminal")).toBe(true);
    expect(hasCapability(c, "session.usage")).toBe(false);
  });

  it("ignores unknown capabilities", () => {
    const c = evaluateCompatibility(
      hello({ revision: 3, min_client_revision: 1, capabilities: ["session.usage", "terminal", "from.the.future", 42 as never] }),
      client(),
    );
    expect(c).toEqual({ status: "compatible", hostRevision: 3, capabilities: ["session.usage", "terminal"] });
  });

  it("asks for chat rewinds, which a Host offers apart from file reverts", () => {
    expect(CLIENT_CAPABILITIES).toContain("session.revert");
    expect(CLIENT_CAPABILITIES).toContain("session.revert.chat");
    const c = evaluateCompatibility(hello({ revision: 3, min_client_revision: 1, capabilities: ["session.revert"] }));
    expect(hasCapability(c, "session.revert")).toBe(true);
    expect(hasCapability(c, "session.revert.chat")).toBe(false);
  });

  it("asks for sub-agent nesting at the revision that brought parent_id", () => {
    expect(CLIENT_CAPABILITIES).toContain("agent.subagents");
    expect(CLIENT_REVISION).toBeGreaterThanOrEqual(4);
    const old = evaluateCompatibility(hello({ revision: 3, min_client_revision: 1, capabilities: ["session.revert"] }));
    expect(hasCapability(old, "agent.subagents")).toBe(false);
  });

  it("asks for background work at revision 5; an older Host is limited, not broken", () => {
    expect(CLIENT_CAPABILITIES).toContain("session.background");
    expect(CLIENT_REVISION).toBeGreaterThanOrEqual(5);
    const old = evaluateCompatibility(hello({ revision: 4, min_client_revision: 1, capabilities: ["agent.subagents"] }));
    expect(old.status).toBe("limited");
    expect(hasCapability(old, "session.background")).toBe(false);
  });

  it("speaks revision 6, which says how a background launch ended", () => {
    expect(CLIENT_REVISION).toBe(6);
    // The field is optional: a revision-5 Host is still compatible.
    expect(evaluateCompatibility(hello({ revision: 5, min_client_revision: 1 })).status).not.toBe("update_required");
  });

  it("a different protocol major is incompatible and names the older side", () => {
    expect(evaluateCompatibility(hello({ protocol: "zwiim/2" }), client())).toMatchObject({
      status: "update_required",
      update: "client",
      reason: "protocol_major",
    });
    expect(evaluateCompatibility(hello({ protocol: "zwiim/0" }), client())).toMatchObject({
      status: "update_required",
      update: "host",
      reason: "protocol_major",
    });
    expect(evaluateCompatibility(hello({ protocol: "nonsense" }), client())).toMatchObject({
      status: "update_required",
      update: "client",
    });
  });

  it("ignores unknown hello fields and malformed revisions", () => {
    const c = evaluateCompatibility(
      { ...hello({ revision: -1 as never, min_client_revision: "2" as never }), future_field: { x: 1 } } as never,
      client({ minHostRevision: 0, capabilities: [] }),
    );
    expect(c).toMatchObject({ status: "compatible", hostRevision: 0 });
  });

  it("this build is compatible with a Host of the same revision", () => {
    expect(evaluateCompatibility(hello()).status).toBe("compatible");
    expect(hasCapability(null, "terminal")).toBe(false);
  });

  it("introduces the client with its revision and capabilities", () => {
    expect(helloClient({ app: "mobile", version: "0.2.0", build: "12", platform: "ios" })).toEqual({
      app: "mobile",
      version: "0.2.0",
      build: "12",
      platform: "ios",
      revision: CLIENT_REVISION,
      capabilities: CLIENT_CAPABILITIES,
    });
  });
});

describe("isHostOutdated", () => {
  it("recognises operations and fields the Host does not know", () => {
    expect(isHostOutdated(new ZwiimClientError("unknown_operation", "x", 404))).toBe(true);
    expect(
      isHostOutdated(new ZwiimClientError("invalid_request", "x", 400, { reason: "unknown_field", field: "new_thing" })),
    ).toBe(true);
    expect(isHostOutdated(new ZwiimClientError("invalid_request", "x", 400))).toBe(false);
    expect(isHostOutdated(new ZwiimClientError("network", "x"))).toBe(false);
    expect(isHostOutdated(new Error("x"))).toBe(false);
  });
});

describe("HostClient compatibility", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  function setup(opts: { hello?: Parameters<typeof hello>[0] } = {}) {
    const transport = new FakeTransport();
    if (opts.hello) transport.handlers["host.hello"] = () => hello(opts.hello);
    const c = new HostClient({
      transport,
      random: () => 0.5,
      client: { app: "desktop", version: "0.2.0", platform: "macos" },
      incompatibleRetryMs: 10_000,
    });
    return { transport, client: c };
  }

  it("introduces itself in host.hello", async () => {
    const t = setup();
    t.client.start();
    await flush();
    expect(t.transport.requests[0]).toMatchObject({
      op: "host.hello",
      params: { client: { app: "desktop", version: "0.2.0", revision: CLIENT_REVISION, capabilities: CLIENT_CAPABILITIES } },
    });
  });

  it("goes live with the verdict in state", async () => {
    const t = setup();
    t.client.start();
    await flush();
    t.transport.channel.open();
    expect(t.client.state).toMatchObject({ status: "live", compat: { status: "compatible" } });
  });

  it("goes live but limited when optional features are missing", async () => {
    const t = setup({ hello: { capabilities: ["terminal"] } });
    t.client.start();
    await flush();
    t.transport.channel.open();
    expect(t.client.state.status).toBe("live");
    expect(t.client.state.compat).toMatchObject({ status: "limited" });
    expect(hasCapability(t.client.state.compat, "session.usage")).toBe(false);
  });

  it("update_required blocks operations and opens no event channel", async () => {
    const t = setup({ hello: { revision: CLIENT_REVISION + 3, min_client_revision: CLIENT_REVISION + 1 } });
    t.client.start();
    await flush();
    expect(t.transport.channels).toHaveLength(0);
    expect(t.client.state).toMatchObject({
      status: "incompatible",
      hostId: "host-1",
      error: { code: "update_required" },
      compat: { status: "update_required", update: "client", requiredClientRevision: CLIENT_REVISION + 1 },
    });
    await expect(t.client.request("workspace.list", {} as never)).rejects.toMatchObject({ code: "update_required" });
    expect(t.transport.count("workspace.list")).toBe(0);
    // The handshake itself stays available, so the app can show the Host's versions.
    await expect(t.client.request("host.hello", {})).resolves.toMatchObject({ host_id: "host-1" });
  });

  it("re-checks slowly and recovers once the other side is updated", async () => {
    const t = setup({ hello: { revision: 0 } });
    t.client.start();
    await flush();
    expect(t.client.state.compat).toMatchObject({ status: "update_required", update: "host" });
    expect(t.transport.count("host.hello")).toBe(1);
    await vi.advanceTimersByTimeAsync(9_999);
    expect(t.transport.count("host.hello")).toBe(1);
    t.transport.handlers["host.hello"] = () => hello();
    await vi.advanceTimersByTimeAsync(1);
    t.transport.channel.open();
    expect(t.client.state).toMatchObject({ status: "live", compat: { status: "compatible" }, error: null });
  });

  it("an old Host can still be asked to update itself", async () => {
    const t = setup({ hello: { revision: 0 } });
    t.transport.handlers["host.update.status"] = () => ({ version: "0.1.0", restart_required: true });
    t.transport.handlers["host.update.apply"] = () => ({ version: "0.2.0", state: "restarting" });
    t.client.start();
    await flush();
    expect(t.client.state.compat).toMatchObject({ status: "update_required", update: "host" });
    await expect(t.client.request("host.update.status", {})).resolves.toMatchObject({ restart_required: true });
    await expect(t.client.request("host.update.apply", { mode: "when_idle" })).resolves.toMatchObject({
      state: "restarting",
    });
    await expect(t.client.request("session.list", {} as never)).rejects.toMatchObject({ code: "update_required" });
  });

  it("stops re-checking when stopped", async () => {
    const t = setup({ hello: { revision: 0 } });
    t.client.start();
    await flush();
    t.client.stop();
    await vi.advanceTimersByTimeAsync(60_000);
    expect(t.transport.count("host.hello")).toBe(1);
    expect(t.client.state.status).toBe("disconnected");
  });
});

describe("unknown events from a newer Host", () => {
  it("an unknown feed event keeps the host state", () => {
    const state = reduceHost(initialHostState(), {
      type: "host_snapshot",
      snapshot: { epoch: "e", seq: 0, workspaces: [], sessions: [], terminals: [] },
    });
    const next = reduceHost(state, {
      type: "host_event",
      event: { seq: 1, type: "workspace.teleported", payload: { anything: true } } as unknown as HostFeedEvent,
    });
    expect(next).toBe(state);
  });

  it("an unknown session event keeps the session view", () => {
    const view = initialSessionView("s1");
    const event = { session_id: "s1", seq: 1, type: "agent.dreamed", payload: {} } as unknown as SessionEvent;
    const next = reduceSession(view, { type: "event", event });
    expect(next).toBeDefined();
    expect(next.live).toEqual(view.live);
    expect(next.agentStatus).toBe(view.agentStatus);
  });
});
