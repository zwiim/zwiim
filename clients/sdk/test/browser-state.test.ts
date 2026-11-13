import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { HostController } from "../src/client/controller";
import { HostClient } from "../src/client/host-client";
import { initialBrowserState, personalTabs, reduceBrowser, sessionTabs } from "../src/state/browser";
import type { BrowserTab } from "../src/types/protocol";
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
});
