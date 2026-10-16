import { ZwiimClientError, type CloneJob, type ConnectionMode } from "@zwiim/sdk";
import { describe, expect, it } from "vitest";
import type { HostEntry } from "../src/account/hosts";
import type { HostConnectionView } from "../src/host/connection";
import { CurrentHost, type CurrentHostState } from "../src/host/current";
import { otherBlockers, removeWorktree, stopAgentError, stopThenRemove } from "../src/host/removal";
import { changeRequest } from "../src/sessions/controls";
import { agentSignInRow, connectionText, hostProblem, hostRowState } from "../src/host/status";
import { MemorySecureStore } from "../src/platform/memory-store";
import { VaultPreferences } from "../src/platform/vault-store";
import { sessionSections } from "../src/sessions/model";
import { startTarget } from "../src/sessions/start";
import { sessionToOpen, worktreeSessions } from "../src/sessions/worktree";
import { MapVault, waitFor, world } from "./support";

function setup(opts: Parameters<typeof world>[0] = {}) {
  const w = world(opts);
  const vault = new MapVault();
  const store = new MemorySecureStore();
  const opened: ConnectionMode[] = [];
  const launch = () =>
    new CurrentHost({
      store,
      prefs: new VaultPreferences(vault),
      open: async (hostId, mode) => {
        if (!(await store.pinnedHost(hostId))) throw new ZwiimClientError("device_refused", "Not paired");
        opened.push(mode);
        return w.connect({ mode });
      },
    });
  const entry: HostEntry = {
    host_id: w.host.hostId,
    name: "Studio Mac",
    static_pubkey: w.host.publicKey,
    online: true,
    last_seen_at: 1,
    paired: true,
  };
  return { w, vault, store, opened, launch, entry, prefs: new VaultPreferences(vault) };
}

/** The worktree kept for the host, as stored. */
const chosenIn = (s: ReturnType<typeof setup>): unknown => JSON.parse(s.vault.items.get("workspaces") ?? "{}")[s.entry.host_id];

const pin = (s: ReturnType<typeof setup>) => s.store.pinHost({ host_id: s.entry.host_id, host_static_pubkey: s.entry.static_pubkey });

describe("the current host", () => {
  it("connects in the host's kept mode, asks who it is, and is remembered", async () => {
    const s = setup({ onLan: false });
    await pin(s);
    await s.prefs.setConnectionMode(s.entry.host_id, "cloud");
    const current = s.launch();
    await current.open(s.entry);
    await waitFor(() => current.state.hello !== null);
    expect(current.state.view).toMatchObject({ mode: "cloud", status: "live", active: "relay" });
    expect(current.state.hello).toMatchObject({ host_id: s.entry.host_id });
    expect(await s.prefs.lastHost()).toEqual({ host_id: s.entry.host_id, name: "Studio Mac" });
    await current.clear();
  });

  it("keeps the connection mode chosen for the host across a restart", async () => {
    const s = setup({ onLan: false });
    await pin(s);
    const first = s.launch();
    await first.open(s.entry);
    await waitFor(() => first.state.view?.status === "live");
    await first.setMode("local");
    expect(first.state.view?.mode).toBe("local");
    // No LAN path: Local only says so, and never falls back to the Cloud.
    await waitFor(() => first.state.view?.error?.code === "lan_unavailable");
    expect(hostProblem(first.state.view, null, "Studio Mac")).toEqual({
      kind: "banner",
      text: "Studio Mac isn't on this network. Switch to Automatic to connect through Zwiim Cloud.",
      action: "switch",
    });
    first.retry();

    // Relaunched: the same host opens in Local only again.
    const again = s.launch();
    await again.open(s.entry);
    expect(again.state.view?.mode).toBe("local");
    expect(s.opened).toEqual(["auto", "local"]);
    await again.setMode("auto");
    await waitFor(() => again.state.view?.status === "live");
    expect(await s.prefs.connectionModes()).toEqual({ [s.entry.host_id]: "auto" });
    await again.clear();
    await first.clear();
  });

  it("keeps one connection: the same host again reuses it, another replaces it", async () => {
    const s = setup();
    await pin(s);
    // Another host of the account (served by the same fake, reached by its own connection).
    const other = { ...s.entry, host_id: "host-2", name: "Office Mac" };
    await s.store.pinHost({ host_id: "host-2", host_static_pubkey: s.entry.static_pubkey });
    const current = s.launch();
    await current.select(s.entry);
    await waitFor(() => current.state.view?.status === "live");
    current.chooseWorkspace("w2");
    await current.select({ ...s.entry, online: false });
    expect(s.opened).toHaveLength(1);
    expect(current.state.workspaceId).toBe("w2");
    expect(current.state.host?.online).toBe(false);
    expect(current.state.view?.status).toBe("live");
    expect(s.w.lan.openLinks).toHaveLength(1);

    // The Host switcher: the bar names the new host at once, not yet connected.
    const switching = current.select(other);
    expect(current.state.host?.name).toBe("Office Mac");
    expect(current.state.view).toBeNull();
    // Workspaces are the host's own: the new one starts from its default.
    expect(current.state.workspaceId).toBeNull();
    await switching;
    expect(s.opened).toHaveLength(2);
    await waitFor(() => current.state.view?.status === "live");
    // The first connection is closed: one link open, the new one.
    await waitFor(() => s.w.lan.openLinks.length === 1);
    await current.clear();
  });

  it("keeps the worktree chosen on each host across a restart", async () => {
    const s = setup();
    await pin(s);
    const first = s.launch();
    await first.open(s.entry);
    await waitFor(() => first.state.view?.status === "live");
    first.chooseWorkspace("wt-1");
    await waitFor(() => chosenIn(s) === "wt-1");
    await first.clear();

    const second = s.launch();
    await second.open(s.entry);
    expect(second.state.workspaceId).toBe("wt-1");
    await second.clear();
  });

  it("shows a workspace it adds or a worktree it creates", async () => {
    const s = setup();
    await pin(s);
    const made = (id: string, over: object = {}) => ({
      workspace_id: id,
      name: id,
      alias: null,
      kind: "root",
      parent_workspace_id: null,
      branch: "main",
      is_git_repo: true,
      base_branch: null,
      base_commit: null,
      removed: false,
      display_path: "~/Projects/" + id,
      ...over,
    });
    const asked: unknown[] = [];
    s.w.host.backend.handlers["workspace.add"] = (p: unknown) => (asked.push(p), made("added"));
    s.w.host.backend.handlers["workspace.create_worktree"] = (p: unknown) => (
      asked.push(p), made("tree", { kind: "worktree", parent_workspace_id: "added", display_path: null })
    );
    const current = s.launch();
    await current.open(s.entry);
    await waitFor(() => current.state.view?.status === "live");

    await current.addWorkspaceAt(["Projects", "added"]);
    await current.addWorkspaceAt(["Projects", "added"], { allowLarge: true });
    expect(current.state.workspaceId).toBe("added");
    await current.createWorktree("added");
    expect(current.state.workspaceId).toBe("tree");
    expect(asked).toEqual([
      { home_path: ["Projects", "added"] },
      { home_path: ["Projects", "added"], allow_large: true },
      { workspace_id: "added" },
    ]);
    await waitFor(() => chosenIn(s) === "tree");
    await current.clear();
  });

  it("continues a merged worktree on a new branch, shown at once, Files staying on it", async () => {
    const s = setup();
    await pin(s);
    const asked: unknown[] = [];
    s.w.host.backend.handlers["workspace.continue"] = (p: unknown) => (
      asked.push(p),
      {
        workspace_id: "tree",
        name: "tree",
        alias: null,
        kind: "worktree",
        parent_workspace_id: "root",
        branch: "zwiim/tree-2",
        is_git_repo: true,
        base_branch: "main",
        base_commit: "abc",
        removed: false,
        display_path: null,
      }
    );
    const current = s.launch();
    await current.open(s.entry);
    await waitFor(() => current.state.view?.status === "live");
    current.chooseWorkspace("tree");

    const moved = await current.continueWorkspace("tree", 12);
    expect(asked).toEqual([{ workspace_id: "tree", number: 12 }]);
    expect(moved.branch).toBe("zwiim/tree-2");
    expect(current.state.store?.getState().workspaces?.find((w) => w.workspace_id === "tree")?.branch).toBe("zwiim/tree-2");
    expect(current.state.workspaceId).toBe("tree");
    await current.clear();
  });

  it("forgets the pairing a host dropped, and asks to pair again", async () => {
    const s = setup({ trusted: false });
    await pin(s);
    const current = s.launch();
    await current.open(s.entry);
    await waitFor(() => current.state.view?.status === "unauthenticated");
    expect(current.state.view?.error?.code).toBe("device_refused");
    await waitFor(() => current.state.host?.paired === false);
    expect(await s.store.pinnedHost(s.entry.host_id)).toBeNull();
    expect(hostProblem(current.state.view, null, "Studio Mac")).toEqual({
      kind: "screen",
      title: "This phone was removed from Studio Mac",
      body: "Pair again to reconnect.",
    });
    await current.clear();
  });

  it("can't open a host this phone isn't paired with", async () => {
    const s = setup();
    const current = s.launch();
    await current.open(s.entry);
    expect(current.state.openError?.code).toBe("device_refused");
    expect(current.state.view).toBeNull();
    expect(hostProblem(null, current.state.openError, "Studio Mac")).toMatchObject({
      kind: "screen",
      title: "This phone isn't paired with Studio Mac",
    });
  });

  it("on sign-out, disconnects and forgets the last host, keeping pairings and modes", async () => {
    const s = setup();
    await pin(s);
    const current = s.launch();
    await current.open(s.entry);
    await current.setMode("cloud");
    await waitFor(() => current.state.view?.status === "live");
    const seen: CurrentHostState[] = [];
    current.subscribe(() => seen.push(current.state));
    await current.clear();
    expect(current.state).toEqual({
      host: null,
      view: null,
      hello: null,
      openError: null,
      store: null,
      workspaceId: null,
      dismissedClones: new Set(),
    });
    expect(await s.prefs.lastHost()).toBeNull();
    expect(await s.prefs.connectionModes()).toEqual({ [s.entry.host_id]: "cloud" });
    expect(await s.store.pinnedHost(s.entry.host_id)).not.toBeNull();
    // Closed: nothing more is reported.
    await new Promise((r) => setTimeout(r, 30));
    expect(seen).toHaveLength(1);
  });
});

describe("how the connection reads", () => {
  const view = (over: Partial<HostConnectionView>): HostConnectionView => ({
    mode: "auto",
    status: "live",
    active: "relay",
    hostId: "h",
    error: null,
    events: 0,
    retryAt: null,
    lanKnown: true,
    compat: null,
    ...over,
  });

  it("shows the path as a pill, never a color, and counts down a reconnect", () => {
    expect(connectionText(view({ active: "lan" }))).toEqual({ status: "Connected locally", pill: "⌂ local", dot: "done" });
    expect(connectionText(view({}))).toEqual({ status: "Connected through Zwiim Cloud", pill: "☁\uFE0E cloud", dot: "done" });
    expect(connectionText(view({ status: "connecting", active: "connecting" })).pill).toBe("connecting…");
    const retrying = view({ status: "reconnecting", active: "reconnecting", retryAt: 10_000 });
    expect(connectionText(retrying, 7_200)).toEqual({ status: "Reconnecting…", pill: "reconnecting · 3s", dot: "work" });
    expect(connectionText(retrying, 10_000).pill).toBe("reconnecting");
    expect(connectionText(view({ status: "error", active: "offline" }))).toEqual({ status: "Offline", pill: "offline", dot: "idle" });
    expect(connectionText(null).pill).toBe("offline");
  });

  it("names which side needs an update, and blocks the Host shell", () => {
    const blocked = (update: "client" | "host") =>
      hostProblem(
        view({
          status: "incompatible",
          active: "offline",
          error: { code: "update_required", message: "m" },
          compat: {
            status: "update_required",
            update,
            reason: update === "client" ? "client_revision" : "host_revision",
            hostRevision: 2,
            requiredClientRevision: update === "client" ? 2 : null,
            requiredHostRevision: update === "host" ? 2 : null,
          },
        }),
        null,
        "Mac",
      );
    expect(blocked("client")).toEqual({
      kind: "update",
      update: "app",
      title: "Update Zwiim",
      body: "Mac needs a newer version of this app.",
    });
    expect(blocked("host")).toMatchObject({ kind: "update", update: "host", title: "Mac needs an update" });
    // Limited is still live: no problem to show.
    expect(
      hostProblem(view({ compat: { status: "limited", hostRevision: 1, capabilities: [], missing: ["terminal"] } }), null, "Mac"),
    ).toBeNull();
  });

  it("names every failure and its next step", () => {
    const failed = (code: string, over: Partial<HostConnectionView> = {}) =>
      hostProblem(view({ status: "reconnecting", active: "reconnecting", error: { code, message: "m" }, ...over }), null, "Mac");
    expect(failed("host_offline")).toEqual({ kind: "banner", text: "Mac is offline. Showing nothing until it's back.", action: "retry" });
    expect(failed("cloud_unavailable", { mode: "cloud" })).toMatchObject({ text: "Zwiim Cloud is unreachable. Local connections still work." });
    // No claim about the direct path while this phone doesn't know where to
    // dial, or in Automatic, where it was just tried and missed.
    expect(failed("cloud_unavailable", { mode: "cloud", lanKnown: false })).toMatchObject({ text: "Zwiim Cloud is unreachable." });
    expect(failed("cloud_unavailable")).toMatchObject({ text: "Zwiim Cloud is unreachable." });
    expect(failed("host_identity_mismatch", { status: "error" })).toMatchObject({ kind: "screen", title: "Couldn't verify Mac" });
    expect(failed("handshake_failed", { status: "error" })).toMatchObject({ kind: "screen", title: "Couldn't verify Mac" });
    expect(failed("unauthenticated", { status: "unauthenticated" })).toMatchObject({ action: "sign_in" });
    // Local only's LAN miss is a banner there; in Automatic the Cloud takes over.
    expect(failed("lan_unavailable", { mode: "local" })).toMatchObject({ text: expect.stringContaining("Mac isn't on this network"), action: "switch" });
    expect(failed("lan_unavailable", { mode: "local", lanKnown: false })).toMatchObject({
      text: "This phone doesn't know where Mac is on the network yet. Switch to Automatic to connect through Zwiim Cloud.",
      action: "switch",
    });
    expect(failed("lan_unavailable")).toBeNull();
    // A retryable failure shows in the pill; a stopped one asks for Retry.
    expect(failed("network")).toBeNull();
    expect(failed("network", { status: "error" })).toMatchObject({ text: "Couldn't connect to Mac. Try again.", action: "retry" });
    expect(hostProblem(view({}), null, "Mac")).toBeNull();
    expect(hostProblem(null, { code: "internal", message: "m" }, "Mac")).toMatchObject({ action: "retry" });
  });

  it("describes the current host's row by this phone's own connection", () => {
    const host: HostEntry = { host_id: "h", name: "Mac", static_pubkey: "pk", online: true, last_seen_at: 100, paired: true };
    const state = (over: Partial<CurrentHostState>): CurrentHostState => ({
      host,
      view: null,
      hello: null,
      openError: null,
      store: null,
      workspaceId: null,
      dismissedClones: new Set(),
      ...over,
    });
    expect(hostRowState(host, state({}), undefined, 200)).toEqual({ meta: "connecting…", dot: "work" });
    expect(hostRowState(host, state({ view: view({ active: "lan" }) }), undefined, 200)).toEqual({
      meta: "⌂ same network · paired",
      dot: "done",
    });
    const offline = { ...host, online: false };
    const retrying = view({ status: "reconnecting", active: "reconnecting" });
    expect(hostRowState(offline, state({ host: offline, view: retrying }), undefined, 7300)).toEqual({
      meta: "offline · last seen 2h ago",
      dot: "idle",
    });
    // Another host: the Cloud's presence.
    const other = { ...host, host_id: "o" };
    expect(hostRowState(other, state({ view: view({ active: "lan" }) }), "local", 200)).toEqual({
      meta: "online · paired",
      dot: "done",
    });
  });
});

describe("how an agent's sign-in reads (read only)", () => {
  const base = { connected: false, method: null, saved_key: false, sign_in: "idle", email: null, plan: null } as const;

  it("names the account, its email and plan", () => {
    expect(
      agentSignInRow({ ...base, agent: "codex", connected: true, method: "account", email: "me@example.com", plan: "ChatGPT Plus" }),
    ).toEqual({ name: "Codex", state: "ChatGPT account", detail: "me@example.com · ChatGPT Plus", dot: "done" });
  });

  it("tells a key, a sign-in in progress and no sign-in apart", () => {
    expect(agentSignInRow({ ...base, agent: "claude_code", connected: true, method: "api_key", saved_key: true })).toEqual({
      name: "Claude",
      state: "API key",
      detail: null,
      dot: "done",
    });
    expect(agentSignInRow({ ...base, agent: "codex", sign_in: "running" }).state).toBe("Signing in…");
    expect(agentSignInRow({ ...base, agent: "codex" })).toMatchObject({ state: "Not signed in", dot: "idle" });
  });
});

describe("starting a session", () => {
  const started = (s: ReturnType<typeof setup>, session_id: string) =>
    s.w.host.backend.requests.some((r) => r.op === "session.history" && (r.params as { session_id: string }).session_id === session_id);

  async function live(s: ReturnType<typeof setup>) {
    await pin(s);
    const current = s.launch();
    await current.open(s.entry);
    await waitFor(() => current.state.view?.status === "live");
    return current;
  }

  it("starts exactly where asked, follows it, and keeps the worktree chosen in Files", async () => {
    const s = setup();
    const asked: unknown[] = [];
    s.w.host.backend.handlers["session.start"] = (p: unknown) => (asked.push(p), { session_id: "s-new" });
    const current = await live(s);
    current.chooseWorkspace("wt-fix");

    const params = {
      workspace_id: "wt-fix",
      agent: "claude_code",
      blocks: [{ type: "text" as const, text: "fix the login bug" }],
      client_message_id: "msg-1",
      options: { model: "opus", effort: "high", mode: "plan" as const },
    };
    expect(await current.startSession(params)).toBe("s-new");
    // Sent as given: the worktree, never its root; every option through.
    expect(asked).toEqual([params]);
    // Followed at once: its transcript is read and its stream attached.
    await waitFor(() => started(s, "s-new"));
    expect(current.state.workspaceId).toBe("wt-fix");
    await current.clear();
  });

  it("answers a request and reports the session a plan was handed off to", async () => {
    const s = setup();
    const asked: unknown[] = [];
    let handedOff: string | undefined;
    s.w.host.backend.handlers["session.respond"] = (p: unknown) => (asked.push(p), handedOff ? { session_id: handedOff } : {});
    const current = await live(s);
    const approve = { kind: "plan_approval" as const, answer: { decision: "approve" as const } };
    expect(await current.respond("s1", "p1", approve)).toBeNull();
    handedOff = "s2";
    const handOff = { kind: "plan_approval" as const, answer: { decision: "hand_off" as const } };
    expect(await current.respond("s1", "p1", handOff)).toBe("s2");
    expect(asked).toEqual([
      { session_id: "s1", request_id: "p1", ...approve },
      { session_id: "s1", request_id: "p1", ...handOff },
    ]);
    await current.clear();
  });

  it("a failed start creates nothing and leaves the selection", async () => {
    const s = setup();
    s.w.host.backend.handlers["session.start"] = () => {
      throw new ZwiimClientError("workspace_removed", "The workspace was removed", 410);
    };
    const current = await live(s);
    current.chooseWorkspace("wt-old");

    await expect(
      current.startSession({ workspace_id: "wt-old", agent: "claude_code", blocks: [{ type: "text", text: "hi" }] }),
    ).rejects.toMatchObject({ code: "workspace_removed" });
    expect(s.w.host.backend.count("session.start")).toBe(1);
    expect(s.w.host.backend.count("session.history")).toBe(0);
    expect(current.state.store?.getState().sessions).toEqual({});
    expect(current.state.workspaceId).toBe("wt-old");
    await current.clear();
  });

  it("can't start before the host is connected", async () => {
    const s = setup();
    const current = s.launch();
    await expect(
      current.startSession({ workspace_id: "a", agent: "claude_code", blocks: [{ type: "text", text: "hi" }] }),
    ).rejects.toMatchObject({ code: "network" });
  });

  it("opening an existing session leaves the worktree chosen in Files", async () => {
    const s = setup();
    const current = await live(s);
    current.chooseWorkspace("wt-fix");
    current.showSession("s-old");
    await waitFor(() => started(s, "s-old"));
    expect(current.state.workspaceId).toBe("wt-fix");
    current.hideSession("s-old");
    expect(current.state.workspaceId).toBe("wt-fix");
    await current.clear();
  });

  it("reads the host's agents again on demand", async () => {
    const s = setup();
    const current = await live(s);
    // The connection's own first read lands before counting.
    await waitFor(() => current.state.store?.getState().agents != null);
    const before = s.w.host.backend.count("agent.list");
    s.w.host.backend.handlers["agent.list"] = () => [
      { agent: "claude_code", name: "Claude Code", available: true, reason: null, options: null, effort_by_model: {}, commands: null },
    ];
    await current.refreshAgents();
    expect(s.w.host.backend.count("agent.list")).toBe(before + 1);
    expect(current.state.store?.getState().agents?.map((a) => a.agent)).toEqual(["claude_code"]);
    await current.clear();
  });
});

describe("several sessions in one worktree (R8.11)", () => {
  const info = (id: string, over: object = {}) => ({
    workspace_id: id,
    name: id,
    alias: null,
    kind: "root",
    parent_workspace_id: null,
    branch: "main",
    is_git_repo: true,
    base_branch: null,
    base_commit: null,
    removed: false,
    display_path: null,
    ...over,
  });
  const tree = (id: string, branch: string, over: object = {}) =>
    info(id, { kind: "worktree", parent_workspace_id: "todo", branch, base_branch: "main", ...over });
  const summary = (session_id: string, workspace_id: string, at: number, title: string) => ({
    session_id,
    workspace_id,
    title,
    agent: "claude_code" as const,
    status: "completed" as const,
    live: false,
    created_at: new Date(1_700_000_000_000 + at * 1000).toISOString(),
    updated_at: new Date(1_700_000_000_000 + at * 1000).toISOString(),
  });

  /** A host with todo, its worktrees fix-auth and feature-ui (one session there), and Files on feature-ui. */
  async function host() {
    const s = setup();
    const WORKSPACES = [info("todo"), tree("fix", "fix-auth"), tree("ui", "feature-ui")];
    s.w.host.backend.handlers["workspace.list"] = () => WORKSPACES;
    s.w.host.backend.handlers["session.list"] = (p: { workspace_id: string }) =>
      p.workspace_id === "ui" ? [summary("u1", "ui", 1, "Refactor UI")] : [];
    // Each start is a new session, announced on the feed as the host does.
    const asked: { workspace_id: string }[] = [];
    let n = 0;
    s.w.host.backend.handlers["session.start"] = (p: { workspace_id: string; blocks: { text: string }[] }) => {
      asked.push(p);
      const session = summary(`s-${++n}`, p.workspace_id, 10 + n, p.blocks[0]!.text);
      setTimeout(() => s.w.host.backend.feedEvent("session.changed", { session }), 0);
      return { session_id: session.session_id };
    };
    await pin(s);
    const current = s.launch();
    await current.open(s.entry);
    await waitFor(() => current.state.store?.getState().workspaces != null);
    current.chooseWorkspace("ui");
    await waitFor(() => chosenIn(s) === "ui");
    const state = () => current.state.store!.getState();
    const start = (workspace_id: string, text: string) =>
      current.startSession({ workspace_id, agent: "claude_code", blocks: [{ type: "text", text }] });
    const ids = (workspace_id: string) => worktreeSessions(state(), workspace_id)?.sessions.map((i) => i.session_id);
    return { s, current, asked, state, start, ids };
  }

  it("starts a new session in the same worktree every time, never reusing one", async () => {
    const { s, current, asked, state, start, ids } = await host();
    const a = await start("fix", "Fix authentication");
    await waitFor(() => ids("fix")?.length === 1);
    const b = await start("fix", "Add tests");
    await waitFor(() => ids("fix")?.length === 2);

    expect(a).not.toBe(b);
    expect(asked.map((p) => p.workspace_id)).toEqual(["fix", "fix"]);
    const fix = worktreeSessions(state(), "fix")!;
    expect(fix.sessions.map((i) => [i.session_id, i.workspace_id, i.title])).toEqual([
      [a, "fix", "Fix authentication"],
      [b, "fix", "Add tests"],
    ]);
    expect(fix.label).toBe("⎇ fix-auth");

    // A third joins them, oldest first; none is replaced.
    const c = await start("fix", "Investigate API error");
    await waitFor(() => ids("fix")?.length === 3);
    expect(ids("fix")).toEqual([a, b, c]);
    expect(s.w.host.backend.count("session.start")).toBe(3);
    await current.clear();
  });

  it("switches A → B → A, and each worktree reopens on its last session", async () => {
    const { current, start, ids } = await host();
    const a = await start("fix", "Fix authentication");
    const b = await start("fix", "Add tests");
    await waitFor(() => ids("fix")?.length === 2);

    // Tabs: each switch shows that session and is remembered for the worktree.
    for (const id of [a, b, a]) {
      current.showSession(id);
      current.rememberSession("fix", id);
      current.hideSession(id);
    }
    expect(current.lastSession("fix")).toBe(a);

    // Another worktree shows its own sessions; fix's are untouched.
    expect(ids("ui")).toEqual(["u1"]);
    expect(sessionToOpen(worktreeSessions(current.state.store!.getState(), "ui")!.sessions, current.lastSession("ui"))).toBe("u1");
    current.rememberSession("ui", "u1");

    // Back to fix: A and B still there, and A (last on screen) reopens.
    expect(ids("fix")).toEqual([a, b]);
    expect(sessionToOpen(worktreeSessions(current.state.store!.getState(), "fix")!.sessions, current.lastSession("fix"))).toBe(a);
    // Never remembered: the newest.
    expect(sessionToOpen(worktreeSessions(current.state.store!.getState(), "fix")!.sessions, null)).toBe(b);
    // A worktree with none opens nothing (New session there instead).
    expect(sessionToOpen(worktreeSessions(current.state.store!.getState(), "todo")!.sessions, null)).toBeNull();
    await current.clear();
  });

  it("forgets the sessions last on screen when another host opens", async () => {
    const { s, current } = await host();
    current.rememberSession("fix", "s-9");
    await current.open(s.entry);
    expect(current.lastSession("fix")).toBe("s-9");
    await current.select({ ...s.entry, host_id: "host-other", name: "Office Mac" });
    expect(current.lastSession("fix")).toBeNull();
    await current.clear();
  });

  it("never moves Files: opening, starting and switching sessions elsewhere", async () => {
    const { s, current, start, ids } = await host();
    // New Session targets fix-auth while Files shows feature-ui.
    expect(startTarget(current.state.store!.getState().workspaces, "fix")).toMatchObject({ kind: "ready", label: "todo · fix-auth" });
    const a = await start("fix", "Fix authentication");
    await waitFor(() => ids("fix")?.length === 1);
    current.showSession(a);
    current.rememberSession("fix", a);
    current.hideSession(a);
    current.showSession("u1");
    current.hideSession("u1");
    expect(current.state.workspaceId).toBe("ui");
    await new Promise((r) => setTimeout(r, 20));
    expect(chosenIn(s)).toBe("ui");
    await current.clear();
  });

  it("creating a workspace or worktree for New Session leaves Files where it was", async () => {
    const { s, current } = await host();
    s.w.host.backend.handlers["workspace.add"] = () => info("notes", { is_git_repo: false, branch: null });
    s.w.host.backend.handlers["workspace.create_worktree"] = () => tree("new-tree", "zwiim/1a2b3c4d");

    const added = await current.addWorkspaceAt(["Projects", "notes"], { choose: false });
    const made = await current.createWorktree("todo", { choose: false });
    expect([added.workspace_id, made.workspace_id]).toEqual(["notes", "new-tree"]);
    expect(current.state.workspaceId).toBe("ui");
    await new Promise((r) => setTimeout(r, 20));
    expect(chosenIn(s)).toBe("ui");
    await current.clear();
  });

  it("keeps every session on the Sessions tab, whichever worktree is on screen", async () => {
    const { current, start, state, ids } = await host();
    const a = await start("fix", "Fix authentication");
    const b = await start("fix", "Add tests");
    await waitFor(() => ids("fix")?.length === 2);
    current.rememberSession("fix", b);
    const listed = sessionSections(state()).recent.map((i) => i.session_id);
    expect(listed.sort()).toEqual([a, b, "u1"].sort());
    await current.clear();
  });

  it("a removed worktree keeps its sessions readable, and nothing new starts there", async () => {
    const { s, current, start, state, ids } = await host();
    const a = await start("fix", "Fix authentication");
    const b = await start("fix", "Add tests");
    await waitFor(() => ids("fix")?.length === 2);
    s.w.host.backend.feedEvent("workspace.changed", { workspace: tree("fix", "fix-auth", { removed: true }) as never });
    await waitFor(() => worktreeSessions(state(), "fix")?.removed === true);
    expect(ids("fix")).toEqual([a, b]);
    expect(startTarget(state().workspaces, "fix")).toEqual({ kind: "removed", label: "todo · fix-auth" });
    expect(current.state.workspaceId).toBe("ui");
    await current.clear();
  });
});

describe("unsent drafts (R8.12)", () => {
  async function live() {
    const s = setup();
    const sent: { session_id: string; text: string }[] = [];
    let fail: ZwiimClientError | null = null;
    s.w.host.backend.handlers["session.send"] = (p: { session_id: string; blocks: { text: string }[] }) => {
      if (fail) throw fail;
      sent.push({ session_id: p.session_id, text: p.blocks[0]!.text });
      return {};
    };
    await pin(s);
    const current = s.launch();
    await current.open(s.entry);
    await waitFor(() => current.state.view?.status === "live");
    return { s, current, sent, failWith: (e: ZwiimClientError | null) => (fail = e) };
  }

  it("keeps each session's draft apart across switching A → B → A and worktrees", async () => {
    const { current } = await live();
    current.chooseWorkspace("ui");
    current.setDraft("a", "fix the login bug");
    current.rememberSession("fix", "a");
    // Switch to B (same worktree) and type there.
    current.setDraft("b", "add tests");
    current.rememberSession("fix", "b");
    // To another worktree's session, and back.
    current.setDraft("u1", "refactor");
    current.rememberSession("ui", "u1");
    current.rememberSession("fix", "a");
    expect([current.draft("a"), current.draft("b"), current.draft("u1")]).toEqual(["fix the login bug", "add tests", "refactor"]);
    expect(current.draft("never-typed")).toBe("");
    // Switching never touches Files.
    expect(current.state.workspaceId).toBe("ui");
    await current.clear();
  });

  it("sending clears only that session's draft", async () => {
    const { current, sent } = await live();
    current.setDraft("a", "  fix it \n");
    current.setDraft("b", "add tests");
    await current.send("a", "fix it", "m-1");
    expect(sent).toEqual([{ session_id: "a", text: "fix it" }]);
    expect(current.draft("a")).toBe("");
    expect(current.draft("b")).toBe("add tests");
    await current.clear();
  });

  it("sends the uploaded files after the text, or alone", async () => {
    const { s, current } = await live();
    const blocks: unknown[] = [];
    s.w.host.backend.handlers["session.send"] = (p: { blocks: unknown[] }) => {
      blocks.push(p.blocks);
      return {};
    };
    const file = { type: "attachment" as const, attachment_id: "att-1" };
    await current.send("a", "read this", "m-1", [file]);
    await current.send("a", "", "m-2", [file]);
    expect(blocks).toEqual([[{ type: "text", text: "read this" }, file], [file]]);
    await current.clear();
  });

  it("keeps the draft when a send fails or its outcome is unknown", async () => {
    const { current, failWith } = await live();
    current.setDraft("a", "fix it");
    failWith(new ZwiimClientError("turn_in_progress", "busy", 409));
    await expect(current.send("a", "fix it", "m-1")).rejects.toMatchObject({ code: "turn_in_progress" });
    expect(current.draft("a")).toBe("fix it");
    failWith(new ZwiimClientError("unknown_outcome", "lost", 0));
    await expect(current.send("a", "fix it", "m-2")).rejects.toMatchObject({ code: "unknown_outcome" });
    expect(current.draft("a")).toBe("fix it");
    await current.clear();
  });

  it("keeps what was typed after the send began", async () => {
    const { current } = await live();
    current.setDraft("a", "fix it");
    const sending = current.send("a", "fix it", "m-1");
    current.setDraft("a", "and also the logout");
    await sending;
    expect(current.draft("a")).toBe("and also the logout");
    await current.clear();
  });

  it("drops drafts when another host opens, keeps them reopening the same one", async () => {
    const { s, current } = await live();
    current.setDraft("a", "fix it");
    await current.open(s.entry);
    expect(current.draft("a")).toBe("fix it");
    await current.select({ ...s.entry, host_id: "host-other", name: "Office Mac" });
    expect(current.draft("a")).toBe("");
    await current.clear();
  });
});

describe("stopping agents and removing worktrees (R8.13)", () => {
  const info = (id: string, over: object = {}) => ({
    workspace_id: id,
    name: id,
    alias: null,
    kind: "root",
    parent_workspace_id: null,
    branch: "main",
    is_git_repo: true,
    base_branch: null,
    base_commit: null,
    removed: false,
    display_path: null,
    ...over,
  });
  const FIX = info("fix", { kind: "worktree", parent_workspace_id: "todo", branch: "fix-auth", base_branch: "main" });

  /** A host with worktree fix-auth where sessions a and b run agents, and Files on fix-auth. */
  async function host() {
    const s = setup();
    const live: Record<string, boolean> = { a: true, b: true };
    let terminals = 0;
    let stopFails: ZwiimClientError | null = null;
    const summary = (session_id: string) => ({
      session_id,
      workspace_id: "fix",
      title: `Session ${session_id}`,
      agent: "claude_code" as const,
      status: "completed" as const,
      live: live[session_id]!,
      created_at: new Date(1_700_000_000_000).toISOString(),
      updated_at: new Date(1_700_000_000_000).toISOString(),
    });
    const b = s.w.host.backend;
    b.handlers["workspace.list"] = () => [info("todo"), FIX];
    b.handlers["session.list"] = (p: { workspace_id: string }) => (p.workspace_id === "fix" ? ["a", "b"].map(summary) : []);
    b.handlers["session.stop"] = (p: { session_id: string }) => {
      if (stopFails) throw stopFails;
      live[p.session_id] = false;
      setTimeout(() => b.feedEvent("session.changed", { session: summary(p.session_id) }), 0);
      return {};
    };
    // The host's guard: running agents or terminals refuse removal, force or not.
    b.handlers["workspace.remove_worktree"] = (p: { workspace_id: string }) => {
      const ids = Object.keys(live).filter((id) => live[id]);
      if (ids.length || terminals) {
        throw new ZwiimClientError("workspace_busy", "busy", 409, { live_sessions: ids.length, terminals, session_ids: ids });
      }
      setTimeout(() => b.feedEvent("workspace.changed", { workspace: { ...FIX, removed: true } as never }), 0);
      return { workspace_id: p.workspace_id, parent_workspace_id: "todo", kept_sessions: 2 };
    };
    await pin(s);
    const current = s.launch();
    await current.open(s.entry);
    await waitFor(() => (current.state.store?.getState().sessionsByWorkspace.fix?.length ?? 0) === 2);
    current.chooseWorkspace("fix");
    const listed = (id: string) => current.state.store!.getState().sessionsByWorkspace.fix?.find((x) => x.session_id === id);
    const calls = () => b.requests.filter((r) => r.op === "session.stop" || r.op === "workspace.remove_worktree").map((r) => [r.op, r.params]);
    return {
      s,
      current,
      live,
      listed,
      calls,
      setTerminals: (n: number) => (terminals = n),
      failStops: (e: ZwiimClientError | null) => (stopFails = e),
    };
  }

  it("changes one session's options one at a time; a new model brings its own effort (R8.15)", async () => {
    const { current, s, listed } = await host();
    const b = s.w.host.backend;
    const effort = { current: "high", values: [{ value: "low", label: "Low", description: null }, { value: "high", label: "High", description: null }] };
    const held: Record<string, { model: string; effort: string | null; mode: "normal" | "plan" | "unattended" }> = {
      a: { model: "opus", effort: "high", mode: "normal" },
      b: { model: "opus", effort: "high", mode: "normal" },
    };
    // The host's agent: Haiku offers no effort, and a model change never keeps another model's.
    const options = (id: string) => ({
      model: { current: held[id]!.model, values: [{ value: "opus", label: "Opus", description: null }, { value: "haiku", label: "Haiku", description: null }] },
      effort: held[id]!.effort === null ? null : { ...effort, current: held[id]!.effort! },
      mode: { current: held[id]!.mode, label: held[id]!.mode, available: ["normal", "plan", "unattended"] as ("normal" | "plan" | "unattended")[] },
    });
    b.handlers["session.set_options"] = (p: { session_id: string; model?: string; effort?: string; mode?: "normal" | "plan" | "unattended" }) => {
      const h = held[p.session_id]!;
      if (p.model) Object.assign(h, { model: p.model, effort: p.model === "haiku" ? null : "high" });
      if (p.effort) {
        if (h.effort === null) throw new ZwiimClientError("option_unavailable", "no", 422, { option: "effort", value: p.effort });
        h.effort = p.effort;
      }
      if (p.mode) h.mode = p.mode;
      return { session_id: p.session_id, options: options(p.session_id) };
    };
    const view = (id: string) => current.state.store!.getState().sessions[id];
    const sent = () => b.requests.filter((r) => r.op === "session.set_options").map((r) => r.params);

    await current.setSessionOptions("a", changeRequest({ model: "haiku" }));
    expect(view("a")?.options?.model?.current).toBe("haiku");
    expect(view("a")?.options?.effort).toBeNull();
    await expect(current.setSessionOptions("a", changeRequest({ effort: "low" }))).rejects.toMatchObject({ code: "option_unavailable" });
    await current.setSessionOptions("a", changeRequest({ model: "opus" }));
    expect(view("a")?.options?.effort?.current).toBe("high");
    await current.setSessionOptions("a", changeRequest({ effort: "low" }));
    await current.setSessionOptions("a", changeRequest({ mode: "plan" }));
    expect(view("a")?.options).toMatchObject({ model: { current: "opus" }, effort: { current: "low" }, mode: { current: "plan" } });
    // Each request names one option: the old effort never rides along with a new model.
    expect(sent()).toEqual([
      { session_id: "a", model: "haiku" },
      { session_id: "a", effort: "low" },
      { session_id: "a", model: "opus" },
      { session_id: "a", effort: "low" },
      { session_id: "a", mode: "plan" },
    ]);
    // The other session in the worktree is untouched, and both stay listed.
    expect(view("b")?.options ?? null).toBeNull();
    expect(held.b).toEqual({ model: "opus", effort: "high", mode: "normal" });
    expect([listed("a")?.live, listed("b")?.live]).toEqual([true, true]);
    await current.clear();
  });

  it("stops one session's agent; it stays listed, not live, and the other keeps running", async () => {
    const { current, listed, s } = await host();
    await current.stopSession("a");
    await waitFor(() => listed("a")?.live === false);
    expect(listed("a")).toMatchObject({ session_id: "a", title: "Session a" });
    expect(listed("b")?.live).toBe(true);
    expect(s.w.host.backend.count("session.stop")).toBe(1);
    await current.clear();
  });

  it("names the blocking sessions, stops them, then removes; Files moves to the workspace", async () => {
    const { current, calls } = await host();
    const first = await removeWorktree(current, "fix");
    expect(first).toEqual({ step: "busy", busy: { live_sessions: 2, terminals: 0, session_ids: ["a", "b"] } });
    if (first.step !== "busy") throw new Error("busy");
    expect(await stopThenRemove(current, "fix", first.busy.session_ids)).toEqual({ step: "removed" });
    expect(calls()).toEqual([
      ["workspace.remove_worktree", { workspace_id: "fix" }],
      ["session.stop", { session_id: "a" }],
      ["session.stop", { session_id: "b" }],
      ["workspace.remove_worktree", { workspace_id: "fix" }],
    ]);
    expect(current.state.workspaceId).toBe("todo");
    await current.clear();
  });

  it("stays blocked while a terminal remains, even forced: the host decides", async () => {
    const { current, setTerminals, calls } = await host();
    setTerminals(1);
    const after = await stopThenRemove(current, "fix", ["a", "b"]);
    expect(after).toEqual({ step: "busy", busy: { live_sessions: 0, terminals: 1, session_ids: [] } });
    if (after.step !== "busy") throw new Error("busy");
    expect(otherBlockers(after.busy)).toBe("1 terminal running");
    expect(await removeWorktree(current, "fix", true)).toMatchObject({ step: "busy" });
    expect(calls().at(-1)).toEqual(["workspace.remove_worktree", { workspace_id: "fix", force: true }]);
    expect(current.state.workspaceId).toBe("fix");
    setTerminals(0);
    expect(await removeWorktree(current, "fix")).toEqual({ step: "removed" });
    await current.clear();
  });

  it("never asks for removal after a stop that failed or may not have happened", async () => {
    const { current, failStops, calls } = await host();
    failStops(new ZwiimClientError("unknown_outcome", "lost", 0));
    await expect(stopThenRemove(current, "fix", ["a", "b"])).rejects.toMatchObject({ code: "unknown_outcome" });
    expect(calls()).toEqual([["session.stop", { session_id: "a" }]]);
    expect(stopAgentError(new ZwiimClientError("unknown_outcome", "lost", 0), "Studio Mac")).toBe(
      "Not sure the agent stopped. Check it on Studio Mac before trying again.",
    );
    await current.clear();
  });

  it("reads an agent not bound to a session yet as starting", () => {
    expect(otherBlockers({ live_sessions: 3, terminals: 2, session_ids: ["a"] })).toBe("2 agents starting · 2 terminals running");
    expect(otherBlockers({ live_sessions: 1, terminals: 0, session_ids: ["a"] })).toBeNull();
  });
});

describe("GitHub clones", () => {
  const job = (over: Partial<CloneJob> = {}): CloneJob => ({
    clone_id: "c1",
    owner: "acme",
    name: "web",
    state: "cloning",
    phase: "receiving",
    percent: 10,
    workspace_id: null,
    worktree_id: null,
    error: null,
    ...over,
  });

  async function live(s: ReturnType<typeof setup>) {
    await pin(s);
    const current = s.launch();
    await current.open(s.entry);
    await waitFor(() => current.state.view?.status === "live");
    return current;
  }

  it("selects the worktree of a clone it started once it is done", async () => {
    const s = setup();
    const asked: unknown[] = [];
    s.w.host.backend.handlers["workspace.clone"] = (p: unknown) => (asked.push(p), { clone_id: "c1" });
    const current = await live(s);
    expect(await current.startClone("acme", "web")).toEqual({ clone_id: "c1" });
    expect(asked).toEqual([{ owner: "acme", name: "web" }]);
    s.w.host.backend.feedEvent("clone.changed", { clone: job({ percent: 80 }) });
    s.w.host.backend.feedEvent("clone.changed", { clone: job({ state: "done", workspace_id: "root", worktree_id: "tree" }) });
    await waitFor(() => current.state.workspaceId === "tree");
    await current.clear();
  });

  it("falls back to the root when a done clone has no worktree", async () => {
    const s = setup();
    s.w.host.backend.handlers["workspace.clone"] = () => ({ clone_id: "c1" });
    const current = await live(s);
    await current.startClone("acme", "web");
    s.w.host.backend.feedEvent("clone.changed", { clone: job({ state: "done", workspace_id: "root" }) });
    await waitFor(() => current.state.workspaceId === "root");
    await current.clear();
  });

  it("still selects when done arrives before the reply", async () => {
    const s = setup();
    s.w.host.backend.handlers["workspace.clone"] = async () => {
      s.w.host.backend.feedEvent("clone.changed", { clone: job({ state: "done", workspace_id: "root", worktree_id: "tree" }) });
      await waitFor(() => current.state.store?.getState().clones.c1?.state === "done");
      return { clone_id: "c1" };
    };
    const current = await live(s);
    await current.startClone("acme", "web");
    await waitFor(() => current.state.workspaceId === "tree");
    await current.clear();
  });

  it("never selects for choose: false, a foreign clone, or a repository already added", async () => {
    const s = setup();
    let reply: object = { clone_id: "c1" };
    s.w.host.backend.handlers["workspace.clone"] = () => reply;
    const current = await live(s);
    current.chooseWorkspace("mine");
    await current.startClone("acme", "web", { choose: false });
    reply = { workspace_id: "root" };
    expect(await current.startClone("acme", "api")).toEqual({ workspace_id: "root" });
    s.w.host.backend.feedEvent("clone.changed", { clone: job({ state: "done", workspace_id: "root", worktree_id: "tree" }) });
    // Started on Desktop: this phone only shows it.
    s.w.host.backend.feedEvent("clone.changed", { clone: job({ clone_id: "c9", state: "done", workspace_id: "r9", worktree_id: "t9" }) });
    await waitFor(() => current.state.store?.getState().clones.c9?.state === "done");
    expect(current.state.workspaceId).toBe("mine");
    await current.clear();
  });

  it("forgets a clone that failed or was cancelled", async () => {
    const s = setup();
    let next = 0;
    s.w.host.backend.handlers["workspace.clone"] = () => ({ clone_id: `c${++next}` });
    const current = await live(s);
    current.chooseWorkspace("mine");
    await current.startClone("acme", "web");
    await current.startClone("acme", "api");
    s.w.host.backend.feedEvent("clone.changed", { clone: job({ clone_id: "c1", state: "failed", error: { code: "network", message: "x" } }) });
    s.w.host.backend.feedEvent("clone.changed", { clone: job({ clone_id: "c2", name: "api", state: "cancelled" }) });
    await waitFor(() => current.state.store?.getState().clones.c2?.state === "cancelled");
    // A later done for the same ids (it can't happen, states only move forward) is not followed.
    s.w.host.backend.feedEvent("clone.changed", { clone: job({ clone_id: "c1", state: "done", workspace_id: "root", worktree_id: "tree" }) });
    await waitFor(() => current.state.store?.getState().clones.c1?.state === "done");
    expect(current.state.workspaceId).toBe("mine");
    await current.clear();
  });

  it("dismisses failed clones on this phone, and switching Host resets what it waits on", async () => {
    const s = setup();
    s.w.host.backend.handlers["workspace.clone"] = () => ({ clone_id: "c1" });
    const current = await live(s);
    await current.startClone("acme", "web");
    current.dismissClone("old");
    current.dismissClone("old");
    expect([...current.state.dismissedClones]).toEqual(["old"]);

    // Reopening the same Host keeps both. Settle it fully before switching
    // away: otherwise this connection's own handshake is abandoned in
    // flight, and its stray `ws.open` can still reach the fake Host after
    // the next connection's, leaving the test feeding events to a connection
    // nothing reads from.
    await current.open(s.entry);
    await waitFor(() => current.state.view?.status === "live");
    expect([...current.state.dismissedClones]).toEqual(["old"]);

    const other = { ...s.entry, host_id: "host-2", name: "Office Mac" };
    await s.store.pinHost({ host_id: other.host_id, host_static_pubkey: other.static_pubkey });
    await current.select(other);
    await waitFor(() => current.state.view?.status === "live");
    expect(current.state.dismissedClones.size).toBe(0);
    current.chooseWorkspace("mine");
    // The fake serves the same backend for every Host id: c1 here is not the
    // clone started before. "live" only means the event channel's `ws.open`
    // handshake finished; it says nothing about `host.subscribe` having
    // reached the fake Host yet. Wait for it, or a `feedEvent` sent the
    // instant we go live can arrive before the client has a snapshot to
    // place it against, and is dropped.
    await waitFor(() => s.w.host.backend.channel.sent.some((m) => m.op === "host.subscribe"));
    s.w.host.backend.feedEvent("clone.changed", { clone: job({ state: "done", workspace_id: "root", worktree_id: "tree" }) });
    await waitFor(() => current.state.store?.getState().clones.c1?.state === "done");
    expect(current.state.workspaceId).toBe("mine");
    await current.clear();
  });

  it("passes list and cancel through to the Host", async () => {
    const s = setup();
    const asked: unknown[] = [];
    s.w.host.backend.handlers["github.repos"] = (p: unknown) => (asked.push(p), { repos: [], next_page: null });
    s.w.host.backend.handlers["workspace.clone_cancel"] = (p: unknown) => (asked.push(p), { cancelled: false });
    const current = await live(s);
    expect(await current.githubRepos(2, true)).toEqual({ repos: [], next_page: null });
    expect(await current.cancelClone("c1")).toBe(false);
    expect(asked).toEqual([{ page: 2, refresh: true }, { clone_id: "c1" }]);
    await current.clear();
  });

  it("refuses clone ops without a connection", async () => {
    const s = setup();
    const current = s.launch();
    for (const op of [() => current.githubRepos(), () => current.startClone("acme", "web"), () => current.cancelClone("c1")]) {
      await expect(op()).rejects.toMatchObject({ code: "network" });
    }
  });
});
