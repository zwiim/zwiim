import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { HostController } from "../src/client/controller";
import { HostClient } from "../src/client/host-client";
import { initialBrowserState, personalTabs, reduceBrowser, sessionTabs, tabUiRequests } from "../src/state/browser";
import type { BrowserTab, BrowserUiRequest } from "../src/types/protocol";
import { FakeTransport, flush, hello } from "./fakes";

const tab = (tab_id: string, position: number, session_id: string | null = "s1"): BrowserTab => ({
  tab_id,
  owner: session_id ? { kind: "session", session_id } : { kind: "personal" },
  url: "about:blank",
  title: "",
  position,
  loading: false,
  discarded: false,
  crashed: false,
});

const dialog = (request_id: string, tab_id: string): BrowserUiRequest => ({
  request_id,
  tab_id,
  owner: { kind: "session", session_id: "s1" },
  kind: "dialog",
  dialog: "confirm",
  message: "Leave?",
  url: "https://example.com/",
});

describe("reduceBrowser", () => {
  it("applies a snapshot, then whole-tab events in each owner's order", () => {
    let s = reduceBrowser(initialBrowserState(), { type: "browser.subscribing" });
    expect(s.status).toBe("loading");
    s = reduceBrowser(s, {
      type: "browser.snapshot",
      tabs: [tab("b", 1), tab("p", 0, null), tab("a", 0)],
      engine: { state: "stopped" },
    });
    expect(s.status).toBe("live");
    expect(sessionTabs(s, "s1").map((t) => t.tab_id)).toEqual(["a", "b"]);
    expect(personalTabs(s).map((t) => t.tab_id)).toEqual(["p"]);

    s = reduceBrowser(s, { type: "browser.tab.state", tab: { ...tab("a", 0), title: "App", loading: true } });
    expect(s.tabs.filter((t) => t.tab_id === "a")).toHaveLength(1);
    expect(sessionTabs(s, "s1")[0]).toMatchObject({ title: "App", loading: true });

    s = reduceBrowser(s, { type: "browser.tab.removed", tab_id: "b", owner: { kind: "session", session_id: "s1" } });
    expect(sessionTabs(s, "s1").map((t) => t.tab_id)).toEqual(["a"]);
    const same = reduceBrowser(s, { type: "browser.tab.removed", tab_id: "gone", owner: { kind: "personal" } });
    expect(same).toBe(s);

    s = reduceBrowser(s, { type: "browser.engine.state", engine: { state: "running", version: "157" } });
    expect(s.engine).toEqual({ state: "running", version: "157" });
  });

  it("follows what tabs wait on until each is done", () => {
    let s = reduceBrowser(initialBrowserState(), {
      type: "browser.snapshot",
      tabs: [tab("a", 0), tab("b", 1)],
      engine: { state: "running", version: "157" },
      ui_requests: [dialog("ui-1", "a")],
    });
    s = reduceBrowser(s, { type: "browser.ui.request", request: dialog("ui-2", "b") });
    s = reduceBrowser(s, { type: "browser.ui.request", request: dialog("ui-2", "b") });
    expect(s.uiRequests.map((r) => r.request_id)).toEqual(["ui-1", "ui-2"]);
    expect(tabUiRequests(s, "b")).toEqual([dialog("ui-2", "b")]);

    s = reduceBrowser(s, { type: "browser.ui.done", request_id: "ui-1", tab_id: "a" });
    expect(tabUiRequests(s, "a")).toEqual([]);
    expect(reduceBrowser(s, { type: "browser.ui.done", request_id: "ui-1", tab_id: "a" })).toBe(s);

    // A snapshot from an older Host has no requests.
    s = reduceBrowser(s, { type: "browser.snapshot", tabs: [], engine: { state: "stopped" } });
    expect(s.uiRequests).toEqual([]);
  });

  it("keeps a refusal until the next subscribe", () => {
    let s = reduceBrowser(initialBrowserState(), {
      type: "browser.error",
      error: { code: "permission_denied", message: "Host only" },
    });
    expect(s).toMatchObject({ status: "refused", error: { code: "permission_denied" } });
    s = reduceBrowser(s, { type: "browser.subscribing" });
    expect(s).toMatchObject({ status: "loading", error: null });
  });
});

describe("browser feed", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  function setup() {
    const transport = new FakeTransport();
    const controller = new HostController(new HostClient({ transport, random: () => 0.5 }));
    return { transport, controller };
  }

  const browserSubscribes = (transport: FakeTransport, i = transport.channels.length - 1) =>
    transport.channels[i]!.sent.filter((m) => m.op === "browser.subscribe").length;

  it("subscribes once watched, and again after a reconnect", async () => {
    const { transport, controller } = setup();
    controller.client.start();
    await flush();
    transport.channel.open();
    expect(browserSubscribes(transport)).toBe(0);

    controller.watchBrowser();
    controller.watchBrowser();
    expect(browserSubscribes(transport)).toBe(1);
    expect(controller.store.getState().browser.status).toBe("loading");
    transport.channel.frame({ type: "browser.snapshot", tabs: [tab("a", 0)], engine: { state: "stopped" } });
    expect(controller.store.getState().browser).toMatchObject({ status: "live", tabs: [{ tab_id: "a" }] });

    transport.channel.drop();
    await vi.advanceTimersByTimeAsync(500);
    transport.channel.open();
    expect(browserSubscribes(transport)).toBe(1);
    // The old tabs stay on screen until the new snapshot replaces them.
    expect(controller.store.getState().browser).toMatchObject({ status: "loading", tabs: [{ tab_id: "a" }] });
  });

  it("does not ask a Host without the browser capability", async () => {
    const { transport, controller } = setup();
    transport.handlers["host.hello"] = () =>
      hello({ capabilities: (hello().capabilities ?? []).filter((c) => c !== "browser") });
    controller.client.start();
    await flush();
    transport.channel.open();
    controller.watchBrowser();
    expect(browserSubscribes(transport)).toBe(0);
    expect(controller.store.getState().browser.status).toBe("off");
  });

  it("stores the tab an operation answers with", async () => {
    const { transport, controller } = setup();
    transport.handlers["browser.tab.open"] = (p) => ({ ...tab("n", 0), url: p.url });
    transport.handlers["browser.tab.navigate"] = (p) => ({ ...tab("n", 0), url: "https://example.com/", loading: p.action === "reload" });
    const opened = await controller.openBrowserTab({ kind: "session", session_id: "s1" }, "https://example.com/");
    expect(opened.url).toBe("https://example.com/");
    await controller.browserTabHistory("n", "reload");
    expect(controller.store.getState().browser.tabs).toEqual([{ ...tab("n", 0), url: "https://example.com/", loading: true }]);
    expect(transport.requests.find((r) => r.op === "browser.tab.open")!.options?.mutating).toBe(true);
  });

  it("answers a dialog once, and receives its requests", async () => {
    const { transport, controller } = setup();
    transport.handlers["browser.ui.answer"] = () => ({});
    controller.client.start();
    await flush();
    transport.channel.open();
    controller.watchBrowser();
    transport.channel.frame({ type: "browser.snapshot", tabs: [tab("a", 0)], engine: { state: "stopped" } });
    transport.channel.frame({ type: "browser.ui.request", request: dialog("ui-1", "a") });
    expect(controller.store.getState().browser.uiRequests).toEqual([dialog("ui-1", "a")]);

    await controller.answerBrowserUi("ui-1", { kind: "dialog", accept: true });
    const sent = transport.requests.find((r) => r.op === "browser.ui.answer")!;
    expect(sent.params).toEqual({ request_id: "ui-1", answer: { kind: "dialog", accept: true } });
    expect(sent.options?.mutating).toBe(true);
    transport.channel.frame({ type: "browser.ui.done", request_id: "ui-1", tab_id: "a" });
    expect(controller.store.getState().browser.uiRequests).toEqual([]);
  });
});
