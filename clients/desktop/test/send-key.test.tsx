import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { HostClient, HostController } from "@zwiim/sdk";
import { HostProvider } from "@/lib/host-context";
import { onPrefsChange, rememberSendKey, sendKey } from "@/lib/prefs";
import { isOtherSendKey, isSendKey, sendShortcut } from "@/lib/send-key";
import { SessionPanel } from "@/panels/SessionPanel";
import { FakeTransport, tick } from "./fakes";

afterEach(cleanup);
beforeEach(() => window.localStorage.clear());

const press = (over: Partial<Parameters<typeof isSendKey>[0]> = {}) => ({
  key: "Enter",
  shiftKey: false,
  altKey: false,
  metaKey: false,
  ctrlKey: false,
  nativeEvent: { isComposing: false },
  ...over,
});

describe("send key", () => {
  it("sends on Enter, or on Mod+Enter alone once chosen", () => {
    expect(isSendKey(press(), "enter", "macos")).toBe(true);
    expect(isSendKey(press({ metaKey: true }), "enter", "macos")).toBe(true);
    expect(isSendKey(press(), "mod_enter", "macos")).toBe(false);
    expect(isSendKey(press({ metaKey: true }), "mod_enter", "macos")).toBe(true);
    expect(isSendKey(press({ ctrlKey: true }), "mod_enter", "macos")).toBe(false);
    expect(isSendKey(press({ ctrlKey: true }), "mod_enter", "windows")).toBe(true);
    expect(isSendKey(press({ metaKey: true }), "mod_enter", "linux")).toBe(false);
  });

  it("leaves Shift+Enter, Alt+Enter, composing and other keys to the text box", () => {
    expect(isSendKey(press({ shiftKey: true }), "enter", "macos")).toBe(false);
    expect(isSendKey(press({ altKey: true }), "enter", "macos")).toBe(false);
    expect(isSendKey(press({ nativeEvent: { isComposing: true } }), "enter", "macos")).toBe(false);
    expect(isSendKey(press({ key: "a" }), "enter", "macos")).toBe(false);
  });

  it("sends the other way on Mod+Shift+Enter only", () => {
    expect(isOtherSendKey(press({ metaKey: true, shiftKey: true }), "macos")).toBe(true);
    expect(isOtherSendKey(press({ ctrlKey: true, shiftKey: true }), "windows")).toBe(true);
    expect(isOtherSendKey(press({ ctrlKey: true, shiftKey: true }), "macos")).toBe(false);
    expect(isOtherSendKey(press({ shiftKey: true }), "macos")).toBe(false);
    expect(isOtherSendKey(press({ metaKey: true }), "macos")).toBe(false);
    expect(isOtherSendKey(press({ metaKey: true, shiftKey: true, altKey: true }), "macos")).toBe(false);
  });

  it("writes the send key the way each platform shows it", () => {
    expect(sendShortcut("enter", "macos")).toBe("Enter");
    expect(sendShortcut("mod_enter", "macos")).toBe("⌘↵");
    expect(sendShortcut("mod_enter", "windows")).toBe("Ctrl+Enter");
  });

  it("is Enter until chosen otherwise, and tells its readers when it changes", () => {
    expect(sendKey()).toBe("enter");
    window.localStorage.setItem("zwiim.desktop.send_key", JSON.stringify("nonsense"));
    expect(sendKey()).toBe("enter");
    let heard = 0;
    const stop = onPrefsChange(() => heard++);
    rememberSendKey("mod_enter");
    expect(sendKey()).toBe("mod_enter");
    expect(heard).toBe(1);
    stop();
    rememberSendKey("enter");
    expect(heard).toBe(1);
  });
});

describe("composer send key", () => {
  async function newSession() {
    const t = new FakeTransport();
    t.handlers["agent.list"] = () => [
      { agent: "claude_code", name: "Claude Code", available: true, reason: null, support: "supported" },
    ];
    t.handlers["session.start"] = () => ({ session_id: "s1" });
    const client = new HostClient({ transport: t });
    const controller = new HostController(client);
    client.start();
    await tick();
    render(
      <HostProvider value={controller}>
        <SessionPanel workspaceId="w1" sessionId={null} onSessionStarted={() => {}} />
      </HostProvider>,
    );
    await tick();
    const box = screen.getByPlaceholderText(/^Describe the task/);
    fireEvent.change(box, { target: { value: "go" } });
    const started = () => t.calls.some((c) => c.op === "session.start");
    return { box, started, dispose: () => controller.dispose() };
  }

  it("sends only on Mod+Enter once that is chosen, even after the composer is open", async () => {
    const { box, started, dispose } = await newSession();
    act(() => rememberSendKey("mod_enter"));
    expect(screen.getByLabelText("Send").getAttribute("title")).toBe("Send (Ctrl+Enter)");
    fireEvent.keyDown(box, { key: "Enter" });
    await tick();
    expect(started()).toBe(false);
    // jsdom is no Mac: the main modifier is Ctrl.
    fireEvent.keyDown(box, { key: "Enter", ctrlKey: true });
    await tick();
    expect(started()).toBe(true);
    dispose();
  });
});
