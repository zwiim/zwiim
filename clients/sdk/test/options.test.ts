import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { HostController } from "../src/client/controller";
import { hostOwnedDetails, optionUnavailableDetails, ZwiimClientError } from "../src/client/errors";
import { HostClient } from "../src/client/host-client";
import { initialSessionView, reduceSession } from "../src/state/session";
import type { AgentSummary, SessionOptions, SessionSnapshot } from "../src/types/protocol";
import { FakeTransport, flush } from "./fakes";

const opts = (mode: "normal" | "plan" | "unattended", model = "sonnet"): SessionOptions => ({
  model: { current: model, values: [{ value: "sonnet", label: "Sonnet", description: null }, { value: "haiku", label: "Haiku", description: null }] },
  effort: null,
  mode: { current: mode, label: mode, available: ["normal", "plan", "unattended"] },
});

const snapshot = (options: SessionOptions | null): SessionSnapshot => ({
  status: "connected",
  live_message: null,
  tool_calls: [],
  pending_request: null,
  pending_user_message: null,
  last_error: null,
  usage: null,
  options,
});

describe("session options state", () => {
  it("follows snapshots and option events, and survives the agent going away", () => {
    let v = initialSessionView("s1");
    v = reduceSession(v, { type: "snapshot", epoch: "e", seq: 1, snapshot: snapshot(opts("plan")) });
    expect(v.options?.mode?.current).toBe("plan");
    v = reduceSession(v, {
      type: "event",
      event: { session_id: "s1", seq: 2, type: "session.options", payload: { options: opts("normal") } },
    });
    expect(v.options?.mode?.current).toBe("normal");
    v = reduceSession(v, { type: "detached", reason: "session_not_live" });
    expect(v.options?.mode?.current).toBe("normal");
    // A respawned agent that has not advertised yet keeps the last known options.
    v = reduceSession(v, { type: "snapshot", epoch: "e2", seq: 0, snapshot: snapshot(null) });
    expect(v.options?.mode?.current).toBe("normal");
    v = reduceSession(v, { type: "options", options: opts("unattended") });
    expect(v.options?.mode?.current).toBe("unattended");
  });

  it("replays option events", () => {
    const v = reduceSession(initialSessionView("s1"), {
      type: "replay",
      epoch: "e",
      events: [{ session_id: "s1", seq: 3, type: "session.options", payload: { options: opts("plan", "haiku") } }],
      high_water_seq: 3,
    });
    expect(v.options?.model?.current).toBe("haiku");
  });
});

describe("controller: agents, options, ownership", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  async function live() {
    const transport = new FakeTransport();
    const client = new HostClient({ transport, random: () => 0.5 });
    const controller = new HostController(client);
    client.start();
    await flush();
    transport.channel.open();
    await flush();
    return { transport, client, controller, store: controller.store };
  }

  it("loads agents on connect", async () => {
    const agents: AgentSummary[] = [
      { agent: "claude_code", name: "Claude Code", available: true, reason: null, options: opts("normal") },
      { agent: "codex", name: "Codex", available: false, reason: "not_installed", options: null },
    ];
    const transport = new FakeTransport();
    transport.handlers["agent.list"] = () => agents;
    const client = new HostClient({ transport, random: () => 0.5 });
    const c = new HostController(client);
    client.start();
    await flush();
    transport.channel.open();
    await flush();
    expect(c.store.getState().agents).toEqual(agents);
  });

  it("reads options on open and on every reconnect", async () => {
    const t = await live();
    t.transport.handlers["session.options"] = (p) => ({ session_id: p.session_id, options: opts("unattended") });
    t.controller.openSession("s1");
    await flush();
    expect(t.store.getState().sessions.s1?.options?.mode?.current).toBe("unattended");
    const before = t.transport.count("session.options");
    t.transport.channel.drop();
    await vi.advanceTimersByTimeAsync(500);
    t.transport.channel.open();
    await flush();
    expect(t.transport.count("session.options")).toBeGreaterThan(before);
  });

  it("drops a read that raced a newer option event", async () => {
    const t = await live();
    let answer!: (v: unknown) => void;
    t.transport.handlers["session.options"] = () => new Promise((r) => (answer = r));
    t.controller.openSession("s1");
    await flush();
    t.transport.channel.frame({ type: "snapshot", subscription_id: "sub-1", session_id: "s1", epoch: "e", snapshot: snapshot(null), event_seq: 1 });
    t.transport.channel.frame({
      type: "event",
      subscription_id: "sub-1",
      event: { session_id: "s1", seq: 2, type: "session.options", payload: { options: opts("plan") } },
    });
    answer({ session_id: "s1", options: opts("normal") });
    await flush();
    expect(t.store.getState().sessions.s1?.options?.mode?.current).toBe("plan");
  });

  it("sets options once, as a mutation, and shows the host's answer", async () => {
    const t = await live();
    t.controller.openSession("s1");
    await flush();
    t.transport.handlers["session.set_options"] = (p) => ({ session_id: p.session_id, options: opts(p.mode) });
    await t.controller.setSessionOptions("s1", { mode: "unattended" });
    const req = t.transport.requests.find((r) => r.op === "session.set_options")!;
    expect(req.params).toEqual({ session_id: "s1", mode: "unattended" });
    expect(req.options?.mutating).toBe(true);
    expect(t.store.getState().sessions.s1?.options?.mode?.current).toBe("unattended");
  });

  it("surfaces an unavailable option with its details and changes nothing", async () => {
    const t = await live();
    t.controller.openSession("s1");
    await flush();
    t.transport.handlers["session.set_options"] = () => {
      throw new ZwiimClientError("option_unavailable", "The agent does not offer this option", 422, { option: "model", value: "x" });
    };
    const err = await t.controller.setSessionOptions("s1", { model: "x" }).catch((e) => e);
    expect(optionUnavailableDetails(err)).toEqual({ option: "model", value: "x" });
    expect(t.store.getState().sessions.s1?.options?.model).toBeNull();
  });

  it("claims the host and shows the owner; another account is refused", async () => {
    const t = await live();
    t.transport.handlers["host.claim"] = (p) => ({ owner: { user_id: p.user_id, email: p.email ?? null } });
    await t.controller.claimHost("u1", "one@example.com");
    expect(t.store.getState().host?.owner).toEqual({ user_id: "u1", email: "one@example.com" });
    t.transport.handlers["host.claim"] = () => {
      throw new ZwiimClientError("host_owned", "owned", 409, { owner_email: "one@example.com" });
    };
    const err = await t.controller.claimHost("u2").catch((e) => e);
    expect(hostOwnedDetails(err)).toEqual({ owner_email: "one@example.com" });
    expect(t.store.getState().host?.owner?.user_id).toBe("u1");
  });

  it("scopes terminals to workspaces", async () => {
    const t = await live();
    t.transport.handlers["terminal.spawn"] = () => ({ terminal_id: "t1" });
    await t.controller.spawnTerminal("w1");
    expect(t.store.getState().terminals.t1?.workspace_id).toBe("w1");
    t.transport.feedEvent("terminal.changed", { terminal: { terminal_id: "t2", title: "zsh", workspace_id: "w2" } });
    expect(t.store.getState().terminals.t2?.workspace_id).toBe("w2");
  });

  it("commits once, as a mutation", async () => {
    const t = await live();
    t.transport.handlers["git.commit"] = () => ({ committed_files: 2 });
    expect(await t.controller.commit("w1", "msg", ["a", "b"])).toBe(2);
    expect(t.transport.requests.find((r) => r.op === "git.commit")?.options?.mutating).toBe(true);
  });
});
