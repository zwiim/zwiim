import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  HostClient,
  HostController,
  type BrowserInput,
  type BrowserStreamHandlers,
  type BrowserTab,
  type BrowserUiRequest,
  ZwiimClientError,
} from "@zwiim/sdk";
import { HostProvider } from "@/lib/host-context";
import { addressUrl, browserShortcut, fitFrame, keyInput, modifiersOf, toPage } from "@/lib/browser-input";
import { BrowserPanel } from "@/panels/BrowserPanel";
import { FakeTransport, tick } from "./fakes";

afterEach(cleanup);

const key = (key: string, more: Partial<KeyboardEvent> = {}) => ({
  key,
  code: more.code ?? "",
  keyCode: more.keyCode ?? 0,
  altKey: more.altKey ?? false,
  ctrlKey: more.ctrlKey ?? false,
  metaKey: more.metaKey ?? false,
  shiftKey: more.shiftKey ?? false,
});

describe("browser input", () => {
  it("counts modifiers as CDP does", () => {
    expect(modifiersOf(key("a", { altKey: true, shiftKey: true }))).toBe(9);
    expect(modifiersOf(key("a", { ctrlKey: true, metaKey: true }))).toBe(6);
  });

  it("fits a frame in its box and maps the pointer back to the page", () => {
    const fit = fitFrame({ width: 400, height: 400 }, { width: 800, height: 600 });
    expect(fit).toEqual({ x: 0, y: 50, width: 400, height: 300 });
    expect(toPage({ x: 200, y: 200 }, fit, { width: 800, height: 600 })).toEqual({ x: 400, y: 300 });
    expect(toPage({ x: 200, y: 20 }, fit, { width: 800, height: 600 })).toBeNull();
    expect(toPage({ x: 1, y: 1 }, fitFrame({ width: 10, height: 10 }, { width: 0, height: 0 }), { width: 0, height: 0 })).toBeNull();
  });

  it("gives typing keys their text, and others none", () => {
    expect(keyInput(key("a", { code: "KeyA", keyCode: 65 }), "down")).toEqual({
      type: "key",
      action: "down",
      key: "a",
      code: "KeyA",
      modifiers: 0,
      text: "a",
      key_code: 65,
    });
    expect(keyInput(key("a", { code: "KeyA" }), "up")).not.toHaveProperty("text");
    expect(keyInput(key("Enter", { code: "Enter", keyCode: 13 }), "down")).not.toHaveProperty("text");
    expect(keyInput(key("a", { metaKey: true }), "down")).not.toHaveProperty("text");
    expect(keyInput(key("é"), "down")).toHaveProperty("text", "é");
  });

  it("keeps the panel's shortcuts and lets other keys through", () => {
    expect(browserShortcut(key("l", { metaKey: true }), "macos")).toBe("address");
    expect(browserShortcut(key("t", { metaKey: true }), "macos")).toBe("new_tab");
    expect(browserShortcut(key("w", { metaKey: true }), "macos")).toBe("close_tab");
    expect(browserShortcut(key("R", { metaKey: true, shiftKey: true }), "macos")).toBe("reload");
    expect(browserShortcut(key("[", { metaKey: true }), "macos")).toBe("back");
    expect(browserShortcut(key("]", { metaKey: true }), "macos")).toBe("forward");
    expect(browserShortcut(key("l", { ctrlKey: true }), "macos")).toBeNull();
    expect(browserShortcut(key("l", { ctrlKey: true }), "windows")).toBe("address");
    expect(browserShortcut(key("a", { metaKey: true }), "macos")).toBeNull();
  });

  it("reads an address bar entry", () => {
    expect(addressUrl("  ")).toBeNull();
    expect(addressUrl("https://example.com/a")).toBe("https://example.com/a");
    expect(addressUrl("about:blank")).toBe("about:blank");
    expect(addressUrl("example.com")).toBe("https://example.com");
    expect(addressUrl("localhost:5173/app")).toBe("http://localhost:5173/app");
    expect(addressUrl("127.0.0.1:3000")).toBe("http://127.0.0.1:3000");
    expect(addressUrl("example.com:8080")).toBe("https://example.com:8080");
  });
});

const tab = (tab_id: string, more: Partial<BrowserTab> = {}): BrowserTab => ({
  tab_id,
  owner: { kind: "session", session_id: "s1" },
  url: "https://example.com/",
  title: "Example",
  position: 0,
  loading: false,
  discarded: false,
  crashed: false,
  ...more,
});

class StreamingTransport extends FakeTransport {
  streams: { id: string; handlers: BrowserStreamHandlers; sent: BrowserInput[]; closed: boolean }[] = [];
  openBrowserStream(id: string, handlers: BrowserStreamHandlers) {
    const s = { id, handlers, sent: [] as BrowserInput[], closed: false };
    this.streams.push(s);
    return { send: (i: BrowserInput) => void s.sent.push(i), close: () => void (s.closed = true) };
  }
}

describe("browser panel", () => {
  beforeEach(() => {
    vi.stubGlobal(
      "ResizeObserver",
      class {
        observe() {}
        disconnect() {}
      },
    );
  });
  afterEach(() => vi.unstubAllGlobals());

  async function mount(t: StreamingTransport, tabs: BrowserTab[], engine: object = { state: "running", version: "157" }) {
    const client = new HostClient({ transport: t });
    const controller = new HostController(client);
    client.start();
    await tick();
    render(
      <HostProvider value={controller}>
        <BrowserPanel sessionId="s1" active />
      </HostProvider>,
    );
    await tick();
    expect(t.sent).toContainEqual({ op: "browser.subscribe" });
    await act(async () => t.frame({ type: "browser.snapshot", tabs, engine } as never));
    await tick();
    return controller;
  }

  it("lists the session's tabs and the person's, and opens a new one for the session", async () => {
    const t = new StreamingTransport();
    t.handlers["browser.view.open"] = () => ({ stream_id: "st1" });
    t.handlers["browser.view.close"] = () => ({});
    t.handlers["browser.tab.open"] = (p) => tab("n", { owner: p.owner, url: "about:blank", title: "", position: 1 });
    await mount(t, [tab("a"), tab("p", { owner: { kind: "personal" }, title: "Mine" }), tab("other", { owner: { kind: "session", session_id: "s2" } })]);
    const tabs = screen.getAllByRole("tab").map((b) => b.textContent);
    expect(tabs).toEqual(["Example", "Mine · Mine"]);

    fireEvent.click(screen.getByTitle("New tab"));
    await tick();
    expect(t.calls.find((c) => c.op === "browser.tab.open")!.params).toEqual({ owner: { kind: "session", session_id: "s1" } });
    expect(screen.getAllByRole("tab").map((b) => b.textContent)).toContain("New tab");
  });

  it("goes to what the address bar says, and steps through history", async () => {
    const t = new StreamingTransport();
    t.handlers["browser.view.open"] = () => ({ stream_id: "st1" });
    t.handlers["browser.view.close"] = () => ({});
    t.handlers["browser.tab.navigate"] = (p) => tab("a", { url: p.url ?? "https://example.com/" });
    await mount(t, [tab("a")]);
    const address = screen.getByLabelText("Address") as HTMLInputElement;
    expect(address.value).toBe("https://example.com/");
    fireEvent.change(address, { target: { value: "localhost:5173" } });
    fireEvent.submit(address.closest("form")!);
    await tick();
    expect(t.calls.find((c) => c.op === "browser.tab.navigate")!.params).toEqual({ tab_id: "a", url: "http://localhost:5173" });
    expect(address.value).toBe("http://localhost:5173");

    fireEvent.click(screen.getByTitle("Back"));
    await tick();
    expect(t.calls.filter((c) => c.op === "browser.tab.navigate").at(-1)!.params).toEqual({ tab_id: "a", action: "back" });
  });

  it("streams the selected tab, sends it keys and keeps the shortcuts", async () => {
    const t = new StreamingTransport();
    t.handlers["browser.view.open"] = () => ({ stream_id: "st1" });
    t.handlers["browser.view.close"] = () => ({});
    t.handlers["browser.tab.navigate"] = () => tab("a");
    await mount(t, [tab("a")]);
    expect(t.calls.find((c) => c.op === "browser.view.open")!.params).toMatchObject({ tab_id: "a", viewport: { width: 100, height: 100 } });
    expect(t.streams.map((s) => s.id)).toEqual(["st1"]);

    const page = screen.getByLabelText("Page: Example");
    fireEvent.keyDown(page, { key: "a", code: "KeyA", keyCode: 65 });
    fireEvent.keyUp(page, { key: "a", code: "KeyA", keyCode: 65 });
    expect(t.streams[0]!.sent).toEqual([
      { type: "key", action: "down", key: "a", code: "KeyA", modifiers: 0, text: "a", key_code: 65 },
      { type: "key", action: "up", key: "a", code: "KeyA", modifiers: 0, key_code: 65 },
    ]);

    const mod = navigator.userAgent.includes("Mac") ? { metaKey: true } : { ctrlKey: true };
    fireEvent.keyDown(page, { key: "r", code: "KeyR", ...mod });
    await tick();
    expect(t.streams[0]!.sent).toHaveLength(2);
    expect(t.calls.find((c) => c.op === "browser.tab.navigate")!.params).toEqual({ tab_id: "a", action: "reload" });

    cleanup();
    await tick();
    expect(t.streams[0]!.closed).toBe(true);
    expect(t.calls.find((c) => c.op === "browser.view.close")!.params).toEqual({ stream_id: "st1" });
  });

  it("explains an engine that could not start, and tries again", async () => {
    const t = new StreamingTransport();
    t.handlers["browser.engine.reset"] = () => ({ engine: { state: "stopped" } });
    await mount(t, [tab("a")], { state: "failed", code: "download_offline", message: "No connection." });
    expect(screen.getByText("The browser could not start.")).toBeTruthy();
    expect(screen.getByText("No connection.")).toBeTruthy();
    expect(t.calls.some((c) => c.op === "browser.view.open")).toBe(false);
    fireEvent.click(screen.getByText("Try again"));
    await tick();
    expect(t.calls.some((c) => c.op === "browser.engine.reset")).toBe(true);
  });

  const dialog = (more: Partial<Extract<BrowserUiRequest, { kind: "dialog" }>> = {}): BrowserUiRequest => ({
    request_id: "ui-1",
    tab_id: "a",
    owner: { kind: "session", session_id: "s1" },
    kind: "dialog",
    dialog: "prompt",
    message: "Your name?",
    default_text: "Ann",
    url: "https://example.com/form",
    ...more,
  });

  it("shows a page's dialog over its tab and sends the answer", async () => {
    const t = new StreamingTransport();
    t.handlers["browser.view.open"] = () => ({ stream_id: "st1" });
    t.handlers["browser.view.close"] = () => ({});
    t.handlers["browser.ui.answer"] = () => ({});
    await mount(t, [tab("a"), tab("b", { title: "Other", position: 1 })]);
    await act(async () => t.frame({ type: "browser.ui.request", request: dialog() } as never));
    expect(screen.getByRole("alertdialog", { name: "example.com says" })).toBeTruthy();
    expect(screen.getByText("Your name?")).toBeTruthy();
    const answer = screen.getByLabelText("Answer") as HTMLInputElement;
    expect(answer.value).toBe("Ann");
    fireEvent.change(answer, { target: { value: "Bo" } });
    fireEvent.submit(answer.closest("form")!);
    await tick();
    expect(t.calls.find((c) => c.op === "browser.ui.answer")!.params).toEqual({
      request_id: "ui-1",
      answer: { kind: "dialog", accept: true, text: "Bo" },
    });
    await act(async () => t.frame({ type: "browser.ui.done", request_id: "ui-1", tab_id: "a" } as never));
    expect(screen.queryByRole("alertdialog")).toBeNull();

    // A dialog in a tab not on screen marks that tab; Escape cancels one on screen.
    await act(async () =>
      t.frame({ type: "browser.ui.request", request: dialog({ request_id: "ui-2", tab_id: "b", dialog: "beforeunload" }) } as never),
    );
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(screen.getByTitle("This page is asking something")).toBeTruthy();
    fireEvent.click(screen.getByText("Other"));
    const leave = screen.getByRole("alertdialog", { name: "Leave this page?" });
    fireEvent.keyDown(leave, { key: "Escape" });
    await tick();
    expect(t.calls.filter((c) => c.op === "browser.ui.answer").at(-1)!.params).toEqual({
      request_id: "ui-2",
      answer: { kind: "dialog", accept: false },
    });
  });

  it("says nothing when another client answered the dialog first", async () => {
    const t = new StreamingTransport();
    t.handlers["browser.view.open"] = () => ({ stream_id: "st1" });
    t.handlers["browser.view.close"] = () => ({});
    t.handlers["browser.ui.answer"] = () => {
      throw new ZwiimClientError("request_not_found", "That request was already answered or has expired");
    };
    await mount(t, [tab("a")]);
    await act(async () => t.frame({ type: "browser.ui.request", request: dialog({ dialog: "alert", message: "Saved" }) } as never));
    fireEvent.click(screen.getByText("OK"));
    await tick();
    expect(t.calls.some((c) => c.op === "browser.ui.answer")).toBe(true);
    expect(screen.queryByText(/already answered/)).toBeNull();
  });

  it("asks for a site's sign-in and sends it, or cancels", async () => {
    const t = new StreamingTransport();
    t.handlers["browser.view.open"] = () => ({ stream_id: "st1" });
    t.handlers["browser.view.close"] = () => ({});
    t.handlers["browser.ui.answer"] = () => ({});
    await mount(t, [tab("a")]);
    const auth = (request_id: string): BrowserUiRequest => ({
      request_id,
      tab_id: "a",
      owner: { kind: "session", session_id: "s1" },
      kind: "auth",
      origin: "http://localhost:8080",
      scheme: "basic",
      realm: "Staging",
      proxy: false,
      url: "http://localhost:8080/admin",
    });
    await act(async () => t.frame({ type: "browser.ui.request", request: auth("ui-1") } as never));
    expect(screen.getByRole("alertdialog", { name: "Sign in" })).toBeTruthy();
    expect(screen.getByText(/http:\/\/localhost:8080 asks for a username and password for “Staging”/)).toBeTruthy();
    fireEvent.change(screen.getByLabelText("Username"), { target: { value: "ann" } });
    const password = screen.getByLabelText("Password") as HTMLInputElement;
    expect(password.type).toBe("password");
    fireEvent.change(password, { target: { value: "pw" } });
    fireEvent.submit(password.closest("form")!);
    await tick();
    expect(t.calls.find((c) => c.op === "browser.ui.answer")!.params).toEqual({
      request_id: "ui-1",
      answer: { kind: "auth", accept: true, username: "ann", password: "pw" },
    });
    await act(async () => t.frame({ type: "browser.ui.done", request_id: "ui-1", tab_id: "a" } as never));

    await act(async () => t.frame({ type: "browser.ui.request", request: { ...auth("ui-2"), proxy: true } } as never));
    fireEvent.click(screen.getByText("Cancel"));
    await tick();
    expect(t.calls.filter((c) => c.op === "browser.ui.answer").at(-1)!.params).toEqual({
      request_id: "ui-2",
      answer: { kind: "auth", accept: false },
    });
    expect(screen.getByRole("alertdialog", { name: "Sign in to the proxy" })).toBeTruthy();
  });

  it("says when the host refuses the browser", async () => {
    const t = new StreamingTransport();
    const client = new HostClient({ transport: t });
    const controller = new HostController(client);
    client.start();
    await tick();
    render(
      <HostProvider value={controller}>
        <BrowserPanel sessionId={null} active />
      </HostProvider>,
    );
    await tick();
    await act(async () => t.frame({ type: "browser.error", error: { code: "permission_denied", message: "Host only" } }));
    expect(screen.getByText(/only available on the host's own computer/)).toBeTruthy();
  });
});
