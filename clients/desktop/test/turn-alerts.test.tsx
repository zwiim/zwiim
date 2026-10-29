import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createHostStore, type HostController, type SessionStatus, type SessionSummary } from "@zwiim/sdk";
import { finishedTurns, indexSessions, turnNotice, useTurnAlerts } from "@/lib/attention";
import type { NativeBridge } from "@/lib/bridge";
import { HostProvider } from "@/lib/host-context";
import { rememberTurnAlerts, turnAlerts } from "@/lib/prefs";
import { TurnNotifyToggle, TurnSoundToggle } from "@/settings/chat";

const chime = vi.hoisted(() => vi.fn());
vi.mock("@/lib/chime", () => ({ playChime: chime }));

afterEach(() => {
  cleanup();
  chime.mockReset();
  vi.restoreAllMocks();
});
beforeEach(() => window.localStorage.clear());

function session(id: string, status: SessionStatus, over: Partial<SessionSummary> = {}): SessionSummary {
  return {
    session_id: id,
    workspace_id: "w1",
    title: `Task ${id}`,
    agent: "claude_code",
    status,
    live: true,
    created_at: "",
    updated_at: "",
    ...over,
  };
}

const index = (...sessions: SessionSummary[]) => indexSessions({ w1: sessions });

describe("finished turns", () => {
  it("finds a turn that went from working to waiting for review or done", () => {
    const before = index(session("a", "in_progress"), session("b", "in_progress"), session("c", "in_progress"));
    const after = index(session("a", "pending_review"), session("b", "completed"), session("c", "in_progress"));
    expect(finishedTurns(before, after).map((s) => s.session_id)).toEqual(["a", "b"]);
  });

  it("says nothing of a stop or an error, a new session, a gone one, or one already finished", () => {
    expect(finishedTurns(index(session("a", "in_progress")), index(session("a", "cancelled")))).toEqual([]);
    expect(finishedTurns(index(), index(session("a", "pending_review")))).toEqual([]);
    expect(finishedTurns(index(session("a", "in_progress")), index())).toEqual([]);
    expect(finishedTurns(index(session("a", "pending_review")), index(session("a", "pending_review")))).toEqual([]);
  });

  it("names the agent and the session in the notification", () => {
    const agents = [{ agent: "claude_code", name: "Claude Code" }] as never;
    expect(turnNotice(session("a", "pending_review"), agents)).toEqual({ title: "Claude Code finished", body: "Task a" });
    expect(turnNotice(session("a", "pending_review", { title: null, agent: null }), null)).toEqual({
      title: "The agent finished",
      body: "Ready for review",
    });
  });
});

describe("turn alerts", () => {
  function watch(notifier?: NativeBridge["notifier"]) {
    const store = createHostStore();
    store.dispatch({ type: "sessions", workspace_id: "w1", sessions: [session("a", "in_progress")] });
    function Watcher() {
      useTurnAlerts({ notifier } as NativeBridge);
      return null;
    }
    render(
      <HostProvider value={{ store } as unknown as HostController}>
        <Watcher />
      </HostProvider>,
    );
    const finish = (status: SessionStatus = "pending_review") =>
      act(() => store.dispatch({ type: "sessions", workspace_id: "w1", sessions: [session("a", status)] }));
    return { finish };
  }

  it("notifies while the window is in the background, and stays quiet by default", () => {
    vi.spyOn(document, "hasFocus").mockReturnValue(false);
    const notify = vi.fn(async () => {});
    watch({ notify }).finish();
    expect(notify).toHaveBeenCalledWith("The agent finished", "Task a");
    expect(chime).not.toHaveBeenCalled();
  });

  it("does not notify the window the user is looking at, or a stopped turn", () => {
    const notify = vi.fn(async () => {});
    vi.spyOn(document, "hasFocus").mockReturnValue(true);
    watch({ notify }).finish();
    vi.spyOn(document, "hasFocus").mockReturnValue(false);
    cleanup();
    watch({ notify }).finish("cancelled");
    expect(notify).not.toHaveBeenCalled();
  });

  it("chimes when chosen, and notifies only when chosen", () => {
    vi.spyOn(document, "hasFocus").mockReturnValue(false);
    rememberTurnAlerts({ notify: false, sound: true });
    const notify = vi.fn(async () => {});
    watch({ notify }).finish();
    expect(chime).toHaveBeenCalledTimes(1);
    expect(notify).not.toHaveBeenCalled();
  });

  it("works without a notifier", () => {
    vi.spyOn(document, "hasFocus").mockReturnValue(false);
    rememberTurnAlerts({ sound: true });
    watch(undefined).finish();
    expect(chime).toHaveBeenCalledTimes(1);
  });
});

describe("turn alert settings", () => {
  const meta = (id: string, title: string) => ({ id, title });

  it("turns notifications off and the sound on, playing it once", () => {
    render(
      <>
        <TurnNotifyToggle setting={meta("turn-notify", "Notify")} />
        <TurnSoundToggle setting={meta("turn-sound", "Sound")} />
      </>,
    );
    const notifyBox = screen.getByTestId("turn-notify") as HTMLInputElement;
    const soundBox = screen.getByTestId("turn-sound") as HTMLInputElement;
    expect([notifyBox.checked, soundBox.checked]).toEqual([true, false]);
    fireEvent.click(notifyBox);
    fireEvent.click(soundBox);
    expect(turnAlerts()).toEqual({ notify: false, sound: true });
    expect([notifyBox.checked, soundBox.checked]).toEqual([false, true]);
    expect(chime).toHaveBeenCalledTimes(1);
  });
});
