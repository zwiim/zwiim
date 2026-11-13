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
import { addressUrl, browserShortcut, fitFrame, fromPage, keyInput, modifiersOf, toPage } from "@/lib/browser-input";
import { BrowserPanel, GhostCursor } from "@/panels/BrowserPanel";
import { DatePicker, FindBar, PageMenu, SelectPicker } from "@/panels/BrowserPageUi";
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

  it("shows where the agent acts, then fades", () => {
    vi.useFakeTimers();
    try {
      const action = { tab_id: "t", kind: "type" as const, x: 50, y: 20, box: { x: 10, y: 10, width: 80, height: 20 } };
      const half = (p: { x: number; y: number }) => ({ x: p.x / 2, y: p.y / 2 });
      render(<GhostCursor action={action} seq={1} place={half} />);
      const ghost = screen.getByTestId("ghost-cursor");
      expect(ghost.className).toContain("opacity-100");
      const ring = ghost.firstElementChild as HTMLElement;
      expect([ring.style.left, ring.style.top, ring.style.width, ring.style.height]).toEqual(["5px", "5px", "40px", "10px"]);
      act(() => vi.advanceTimersByTime(1500));
      expect(ghost.className).toContain("opacity-0");
    } finally {
      vi.useRealTimers();
    }
  });

  it("fits a frame in its box and maps the pointer back to the page", () => {
    const fit = fitFrame({ width: 400, height: 400 }, { width: 800, height: 600 });
    expect(fit).toEqual({ x: 0, y: 50, width: 400, height: 300 });
    expect(toPage({ x: 200, y: 200 }, fit, { width: 800, height: 600 })).toEqual({ x: 400, y: 300 });
    expect(toPage({ x: 200, y: 20 }, fit, { width: 800, height: 600 })).toBeNull();
    expect(fromPage({ x: 400, y: 300 }, fit, { width: 800, height: 600 })).toEqual({ x: 200, y: 200 });
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

describe("what the page does not paint", () => {
  const same = (p: { x: number; y: number }) => p;

  it("offers the menu for what was right-clicked, and the page's history", () => {
    const onAction = vi.fn();
    const onClose = vi.fn();
    const menu = { tab_id: "a", x: 30, y: 40, link_url: "https://a.test/docs", selection: "hello", editable: false };
    render(<PageMenu menu={menu} place={same} view={null} onAction={onAction} onClose={onClose} />);
    const items = screen.getAllByRole("menuitem").map((i) => i.textContent);
    expect(items).toEqual(["Open link in new tab", "Copy link address", "Copy", "Back", "Forward", "Reload"]);
    fireEvent.click(screen.getByText("Copy link address"));
    expect(onAction).toHaveBeenCalledWith({ kind: "copy", text: "https://a.test/docs" });
    expect(onClose).toHaveBeenCalled();
  });

  it("lists a select's options and picks one by click or keys", () => {
    const onAnswer = vi.fn();
    const request = {
      request_id: "ui-1",
      tab_id: "a",
      owner: { kind: "personal" as const },
      kind: "select" as const,
      x: 0,
      y: 0,
      width: 200,
      height: 30,
      options: [
        { index: 0, label: "Apple", disabled: false, selected: true },
        { index: 1, label: "Banana", disabled: false, selected: false, group: "More" },
        { index: 2, label: "Cherry", disabled: true, selected: false, group: "More" },
      ],
    };
    render(<SelectPicker request={request} place={same} view={null} onAnswer={onAnswer} />);
    expect(screen.getByText("More")).toBeTruthy();
    expect((screen.getByRole("option", { name: "Cherry" }) as HTMLButtonElement).disabled).toBe(true);
    const list = screen.getByRole("listbox");
    fireEvent.keyDown(list, { key: "ArrowDown" });
    fireEvent.keyDown(list, { key: "ArrowDown" });
    fireEvent.keyDown(list, { key: "Enter" });
    expect(onAnswer).toHaveBeenLastCalledWith({ kind: "select", accept: true, index: 1 });
    fireEvent.click(screen.getByRole("option", { name: "Apple" }));
    expect(onAnswer).toHaveBeenLastCalledWith({ kind: "select", accept: true, index: 0 });
    fireEvent.keyDown(list, { key: "Escape" });
    expect(onAnswer).toHaveBeenLastCalledWith({ kind: "select", accept: false });
  });

  it("picks a date, or clears it", () => {
    const onAnswer = vi.fn();
    const request = {
      request_id: "ui-2",
      tab_id: "a",
      owner: { kind: "personal" as const },
      kind: "date" as const,
      input_type: "date" as const,
      value: "2026-11-13",
      x: 0,
      y: 0,
      width: 200,
      height: 30,
    };
    render(<DatePicker request={request} place={same} view={null} onAnswer={onAnswer} />);
    const input = screen.getByLabelText("Value") as HTMLInputElement;
    expect(input.value).toBe("2026-11-13");
    fireEvent.change(input, { target: { value: "2026-12-25" } });
    fireEvent.click(screen.getByText("Done"));
    expect(onAnswer).toHaveBeenLastCalledWith({ kind: "date", accept: true, value: "2026-12-25" });
    fireEvent.click(screen.getByText("Clear"));
    expect(onAnswer).toHaveBeenLastCalledWith({ kind: "date", accept: true, value: "" });
  });

  it("finds as the person types and steps through the matches", async () => {
    vi.useFakeTimers();
    try {
      const find = vi.fn(async () => ({ matches: 3, current: 1 }));
      const onClose = vi.fn();
      render(<FindBar find={find} onClose={onClose} />);
      const input = screen.getByLabelText("Find in page");
      fireEvent.change(input, { target: { value: "apple" } });
      await act(async () => vi.advanceTimersByTimeAsync(200));
      expect(find).toHaveBeenCalledWith("apple", false);
      expect(screen.getByText("1/3")).toBeTruthy();
      fireEvent.keyDown(input, { key: "Enter", shiftKey: true });
      expect(find).toHaveBeenLastCalledWith("apple", true);
      fireEvent.keyDown(input, { key: "Escape" });
      expect(onClose).toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });
});

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
        <BrowserPanel workspaceId="w1" sessionId="s1" active />
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

  it("opens find in page with the shortcut and clears it on close", async () => {
    const t = new StreamingTransport();
    t.handlers["browser.view.open"] = () => ({ stream_id: "st1" });
    t.handlers["browser.view.close"] = () => ({});
    t.handlers["browser.tab.find"] = (p) => (p.query ? { matches: 2, current: 1 } : { matches: 0, current: 0 });
    await mount(t, [tab("a")]);
    const mod = navigator.userAgent.includes("Mac") ? { metaKey: true } : { ctrlKey: true };
    fireEvent.keyDown(screen.getByLabelText("Page: Example"), { key: "f", code: "KeyF", ...mod });
    const input = screen.getByLabelText("Find in page");
    fireEvent.change(input, { target: { value: "apple" } });
    await act(async () => new Promise((r) => setTimeout(r, 200)));
    expect(t.calls.find((c) => c.op === "browser.tab.find")!.params).toEqual({ tab_id: "a", query: "apple", backwards: false });
    expect(screen.getByText("1/2")).toBeTruthy();
    fireEvent.click(screen.getByTitle("Close"));
    await tick();
    expect(screen.queryByLabelText("Find in page")).toBeNull();
    expect(t.calls.filter((c) => c.op === "browser.tab.find").at(-1)!.params).toEqual({ tab_id: "a", query: "", backwards: false });
  });

  it("draws a select's list over the page, not as a sheet", async () => {
    const t = new StreamingTransport();
    t.handlers["browser.view.open"] = () => ({ stream_id: "st1" });
    t.handlers["browser.view.close"] = () => ({});
    await mount(t, [tab("a")]);
    const request = {
      request_id: "ui-9",
      tab_id: "a",
      owner: { kind: "session", session_id: "s1" },
      kind: "select",
      x: 0,
      y: 0,
      width: 100,
      height: 20,
      options: [{ index: 0, label: "One", disabled: false, selected: true }],
    };
    await act(async () => t.frame({ type: "browser.ui.request", request } as never));
    expect(screen.queryByRole("alertdialog")).toBeNull();
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

  it("asks whether the agent may open a site, and sends the answer", async () => {
    const t = new StreamingTransport();
    t.handlers["browser.view.open"] = () => ({ stream_id: "st1" });
    t.handlers["browser.view.close"] = () => ({});
    t.handlers["browser.ui.answer"] = () => ({});
    await mount(t, [tab("a")]);
    const site = (request_id: string, local = false): BrowserUiRequest => ({
      request_id,
      tab_id: "a",
      owner: { kind: "session", session_id: "s1" },
      kind: "site",
      site: local ? "127.0.0.1:9000" : "docs.example.com:443",
      url: local ? "http://127.0.0.1:9000/admin" : "https://docs.example.com/guide",
      local,
    });
    const answers = () => t.calls.filter((c) => c.op === "browser.ui.answer").map((c) => c.params);

    await act(async () => t.frame({ type: "browser.ui.request", request: site("ui-1") } as never));
    expect(screen.getByRole("alertdialog", { name: "Let the agent open docs.example.com?" })).toBeTruthy();
    expect(screen.getByText("https://docs.example.com/guide")).toBeTruthy();
    expect(screen.queryByText(/your computer or local network/)).toBeNull();
    fireEvent.click(screen.getByText("Always allow"));
    await tick();
    await act(async () => t.frame({ type: "browser.ui.done", request_id: "ui-1", tab_id: "a" } as never));

    await act(async () => t.frame({ type: "browser.ui.request", request: site("ui-2", true) } as never));
    expect(screen.getByRole("alertdialog", { name: "Let the agent open 127.0.0.1:9000?" })).toBeTruthy();
    expect(screen.getByText(/your computer or local network/)).toBeTruthy();
    fireEvent.click(screen.getByText("Deny"));
    await tick();
    expect(answers()).toEqual([
      { request_id: "ui-1", answer: { kind: "site", allow: true, always: true } },
      { request_id: "ui-2", answer: { kind: "site", allow: false } },
    ]);
  });

  it("keeps the session's agent to the sites the person lists", async () => {
    const t = new StreamingTransport();
    t.handlers["browser.view.open"] = () => ({ stream_id: "st1" });
    t.handlers["browser.view.close"] = () => ({});
    t.handlers["browser.sites.get"] = () => ({ sites: ["github.com"] });
    t.handlers["browser.sites.set"] = (p) => {
      if (p.sites.includes("bad site")) throw new ZwiimClientError("invalid_site", "\"bad site\" is not a site");
      return { sites: p.sites.map((s: string) => s.replace(/^https:\/\/|\/.*$/g, "")) };
    };
    await mount(t, [tab("a")]);
    expect(t.calls.find((c) => c.op === "browser.sites.get")!.params).toEqual({ session_id: "s1" });
    const button = screen.getByTitle("Sites the agent may open");
    expect(button.textContent).toBe("1");
    fireEvent.click(button);
    const dialog = screen.getByRole("dialog", { name: "Sites the agent may open" });
    expect(dialog.textContent).toContain("opens only these sites");
    const input = screen.getByLabelText("Add a site") as HTMLInputElement;
    fireEvent.change(input, { target: { value: "https://docs.rs/serde" } });
    fireEvent.submit(input.closest("form")!);
    await tick();
    expect(t.calls.filter((c) => c.op === "browser.sites.set").at(-1)!.params).toEqual({
      session_id: "s1",
      sites: ["github.com", "https://docs.rs/serde"],
    });
    expect(screen.getByText("docs.rs")).toBeTruthy();
    expect(input.value).toBe("");

    fireEvent.change(input, { target: { value: "bad site" } });
    fireEvent.submit(input.closest("form")!);
    await tick();
    expect(screen.getByRole("alert").textContent).toContain("is not a site");
    expect(input.value).toBe("bad site");

    fireEvent.click(screen.getByLabelText("Remove github.com"));
    await tick();
    expect(t.calls.filter((c) => c.op === "browser.sites.set").at(-1)!.params).toEqual({ session_id: "s1", sites: ["docs.rs"] });
    expect(screen.queryByText("github.com")).toBeNull();
  });

  it("picks workspace files for a page's file input", async () => {
    const t = new StreamingTransport();
    t.handlers["browser.view.open"] = () => ({ stream_id: "st1" });
    t.handlers["browser.view.close"] = () => ({});
    t.handlers["browser.ui.answer"] = () => ({});
    t.handlers["workspace.tree"] = (p) =>
      p.path === "assets"
        ? [{ kind: "file", name: "logo.png", path: "assets/logo.png" }]
        : [
            { kind: "file", name: "README.md", path: "README.md" },
            { kind: "dir", name: "assets", path: "assets", children: [] },
          ];
    await mount(t, [tab("a")]);
    await act(async () =>
      t.frame({
        type: "browser.ui.request",
        request: {
          request_id: "ui-1",
          tab_id: "a",
          owner: { kind: "session", session_id: "s1" },
          kind: "file_chooser",
          multiple: true,
          url: "https://example.com/upload",
        },
      } as never),
    );
    await tick();
    expect(screen.getByRole("alertdialog", { name: "Choose files" })).toBeTruthy();
    // Folders first.
    expect(screen.getAllByRole("listitem").map((li) => li.textContent)).toEqual(["assets", "README.md"]);
    expect((screen.getByText("Choose") as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(screen.getByText("README.md"));
    fireEvent.click(screen.getByText("assets"));
    await tick();
    expect(t.calls.filter((c) => c.op === "workspace.tree").at(-1)!.params).toEqual({ workspace_id: "w1", path: "assets", max_depth: 1 });
    fireEvent.click(screen.getByText("logo.png"));
    expect(screen.getByText("2 files")).toBeTruthy();
    fireEvent.click(screen.getByText("Choose"));
    await tick();
    expect(t.calls.find((c) => c.op === "browser.ui.answer")!.params).toEqual({
      request_id: "ui-1",
      answer: { kind: "file_chooser", accept: true, workspace_id: "w1", paths: ["README.md", "assets/logo.png"] },
    });
  });

  it("says when the host refuses the browser", async () => {
    const t = new StreamingTransport();
    const client = new HostClient({ transport: t });
    const controller = new HostController(client);
    client.start();
    await tick();
    render(
      <HostProvider value={controller}>
        <BrowserPanel workspaceId="w1" sessionId={null} active />
      </HostProvider>,
    );
    await tick();
    await act(async () => t.frame({ type: "browser.error", error: { code: "permission_denied", message: "Host only" } }));
    expect(screen.getByText(/only available on the host's own computer/)).toBeTruthy();
  });
});
