import { useRef, useState } from "react";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { VoiceContext, type VoiceBridge, type VoiceEvents } from "@/lib/bridge";
import { rememberVoice } from "@/lib/prefs";
import { Dictation } from "@/panels/Dictation";
import { type OpenSettings, OpenSettingsContext } from "@/settings/open";

function mockVoice() {
  const events = new Map<string, (p: never) => void>();
  const b: VoiceBridge = {
    models: vi.fn(async () => []),
    download: vi.fn(async () => {}),
    delete: vi.fn(async () => {}),
    inputs: vi.fn(async () => []),
    start: vi.fn(async () => {}),
    stop: vi.fn(async () => "um fix swim"),
    cancel: vi.fn(async () => {}),
    unloadAfter: vi.fn(async () => {}),
    listen: vi.fn(async (name, cb) => {
      events.set(name, cb as (p: never) => void);
      return () => {
        events.delete(name);
      };
    }),
  };
  return {
    b,
    emit<K extends keyof VoiceEvents>(name: K, p: VoiceEvents[K]) {
      act(() => events.get(name)?.(p as never));
    },
  };
}
function Composer({ b, open = vi.fn() }: { b?: VoiceBridge; open?: OpenSettings }) {
  const area = useRef<HTMLTextAreaElement>(null);
  const [text, setText] = useState("hello world");
  return (
    <VoiceContext.Provider value={b}>
      <OpenSettingsContext.Provider value={open}>
        <div data-testid="composer">
          <textarea aria-label="Composer" ref={area} value={text} onChange={(e) => setText(e.target.value)} />
          <Dictation area={area} value={text} onChange={setText} />
        </div>
      </OpenSettingsContext.Provider>
    </VoiceContext.Provider>
  );
}
async function ready(b: VoiceBridge, open = vi.fn()) {
  const view = render(<Composer b={b} open={open} />);
  await waitFor(() =>
    expect((screen.getByRole("button", { name: "Dictate" }) as HTMLButtonElement).disabled).toBe(false),
  );
  const area = screen.getByRole("textbox") as HTMLTextAreaElement;
  area.focus();
  area.setSelectionRange(5, 5);
  return { ...view, area };
}
const down = () => fireEvent.keyDown(window, { key: "e", metaKey: true, ctrlKey: true });
const up = () => fireEvent.keyUp(window, { key: "e" });
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("composer dictation", () => {
  it("hides the mic outside the native shell and when disabled in preferences", () => {
    const view = render(<Composer />);
    expect(screen.queryByRole("button", { name: "Dictate" })).toBeNull();
    rememberVoice({ enabled: false });
    view.rerender(<Composer b={mockVoice().b} />);
    expect(screen.queryByRole("button", { name: "Dictate" })).toBeNull();
  });
  it("tap toggles, renders live text and inserts cleaned words at the caret", async () => {
    rememberVoice({ vocabulary: [{ word: "Zwiim", heardAs: "swim" }] });
    const { b, emit } = mockVoice();
    const { area } = await ready(b);
    down();
    up();
    await waitFor(() =>
      expect(b.start).toHaveBeenCalledWith({
        model: "parakeet-tdt-v3",
        language: "auto",
        input: "",
        gpu: true,
        silence_secs: 60,
      }),
    );
    await waitFor(() => expect(screen.getByText("Listening…")).toBeTruthy());
    expect(b.stop).not.toHaveBeenCalled();
    emit("partial", { text: "fix swim" });
    emit("level", { rms: 0.1 });
    expect(screen.getByText("fix swim")).toBeTruthy();
    down();
    up();
    await waitFor(() => expect(area.value).toBe("hello fix Zwiim world"));
    await waitFor(() => expect(document.activeElement).toBe(area));
    expect(b.stop).toHaveBeenCalledTimes(1);
  });
  it("hold waits for a slow microphone start, then stops on release", async () => {
    rememberVoice({ mode: "hold" });
    const { b } = mockVoice();
    let resolveStart!: () => void;
    b.start = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          resolveStart = resolve;
        }),
    );
    const { area } = await ready(b);
    down();
    await waitFor(() => expect(b.start).toHaveBeenCalledTimes(1));
    up();
    expect(b.stop).not.toHaveBeenCalled();
    await act(async () => {
      resolveStart();
    });
    await waitFor(() => expect(b.stop).toHaveBeenCalledTimes(1));
    expect(area.value).toBe("hello fix swim world");
  });
  it("auto switches a hold longer than 400 ms to push-to-talk", async () => {
    const { b } = mockVoice();
    await ready(b);
    let now = 1000;
    vi.spyOn(performance, "now").mockImplementation(() => now);
    down();
    await waitFor(() => expect(screen.getByText("Listening…")).toBeTruthy());
    now += 401;
    up();
    await waitFor(() => expect(b.stop).toHaveBeenCalledTimes(1));
  });
  it("toggle keeps listening on release, and Escape drops the recording", async () => {
    rememberVoice({ mode: "toggle" });
    const { b } = mockVoice();
    const { area } = await ready(b);
    down();
    up();
    await waitFor(() => expect(screen.getByText("Listening…")).toBeTruthy());
    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(b.cancel).toHaveBeenCalledTimes(1));
    expect(b.stop).not.toHaveBeenCalled();
    expect(area.value).toBe("hello world");
  });
  it("finishes automatically on silence and cancels on microphone errors", async () => {
    const { b, emit } = mockVoice();
    const { area } = await ready(b);
    down();
    up();
    await waitFor(() => expect(screen.getByText("Listening…")).toBeTruthy());
    emit("stopped", { reason: "silence" });
    await waitFor(() => expect(b.stop).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(area.value).toBe("hello fix swim world"));
    down();
    up();
    await waitFor(() => expect(screen.getByText("Listening…")).toBeTruthy());
    emit("stopped", { reason: "error", error: "Microphone disconnected." });
    await waitFor(() => expect(b.cancel).toHaveBeenCalledTimes(1));
    expect(screen.getByRole("alert").textContent).toContain("Microphone disconnected.");
  });
  it("keeps a model error visible after releasing Hold and opens Voice settings", async () => {
    rememberVoice({ mode: "hold" });
    const { b } = mockVoice();
    const open = vi.fn();
    b.start = vi.fn(async () => {
      throw new Error("Download Parakeet TDT v3 in Settings › Voice first.");
    });
    await ready(b, open);
    down();
    up();
    expect(await screen.findByRole("alert")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Open Voice settings" }));
    expect(open).toHaveBeenCalledWith({ tab: "voice" });
  });
  it("closes a microphone that finished opening after the composer unmounted", async () => {
    const { b } = mockVoice();
    let resolveStart!: () => void;
    b.start = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          resolveStart = resolve;
        }),
    );
    const view = await ready(b);
    down();
    await waitFor(() => expect(b.start).toHaveBeenCalledTimes(1));
    view.unmount();
    await act(async () => {
      resolveStart();
    });
    await waitFor(() => expect(b.cancel).toHaveBeenCalledTimes(1));
  });
  it("starts from the mic button and inserts with the checkmark", async () => {
    const { b } = mockVoice();
    const { area } = await ready(b);
    const mic = screen.getByRole("button", { name: "Dictate" });
    Object.defineProperty(mic, "setPointerCapture", { value: vi.fn() });
    fireEvent(mic, new MouseEvent("pointerdown", { bubbles: true, button: 0 }));
    fireEvent(mic, new MouseEvent("pointerup", { bubbles: true, button: 0 }));
    await waitFor(() => expect(screen.getByText("Listening…")).toBeTruthy());
    fireEvent.click(screen.getByRole("button", { name: "Insert dictation" }));
    await waitFor(() => expect(area.value).toBe("hello fix swim world"));
  });
  it("drops a final decode if Escape arrives while it is transcribing", async () => {
    const { b } = mockVoice();
    let resolveStop!: (text: string) => void;
    b.stop = vi.fn(
      () =>
        new Promise<string>((resolve) => {
          resolveStop = resolve;
        }),
    );
    const { area } = await ready(b);
    down();
    up();
    await waitFor(() => expect(screen.getByText("Listening…")).toBeTruthy());
    fireEvent.click(screen.getByRole("button", { name: "Insert dictation" }));
    await waitFor(() => expect(b.stop).toHaveBeenCalledTimes(1));
    fireEvent.keyDown(window, { key: "Escape" });
    await act(async () => {
      resolveStop("discard these words");
    });
    expect(area.value).toBe("hello world");
  });
  it("leaves shortcuts in other text fields alone", async () => {
    const { b } = mockVoice();
    await ready(b);
    const input = document.createElement("input");
    document.body.append(input);
    input.focus();
    down();
    up();
    expect(b.start).not.toHaveBeenCalled();
    input.remove();
  });
});
