import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { tauriBridge, type NativeBridge, type VoiceBridge, type VoiceModel } from "@/lib/bridge";
import { voice } from "@/lib/prefs";
import {
  registryProblems,
  SETTINGS,
  TABS,
  visibleSettings,
  visibleTabs,
  type SettingsContext,
} from "@/settings/registry";
import { searchSettings } from "@/settings/search";

const native = vi.hoisted(() => ({
  invoke: vi.fn(async () => 42),
  channels: [] as { onmessage?: (value: unknown) => void }[],
}));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: native.invoke,
  Channel: class {
    constructor() {
      native.channels.push(this);
    }
  },
}));

const models: VoiceModel[] = [
  {
    id: "parakeet-tdt-v3",
    label: "Parakeet TDT v3",
    description: "Fast. 25 European languages.",
    quality: "Very good",
    recommended: true,
    size_mb: 487.211443,
    installed: false,
    downloading: false,
  },
  {
    id: "whisper-tiny",
    label: "Whisper Tiny",
    description: "Smallest download.",
    quality: "Fair",
    recommended: false,
    size_mb: 116.204861,
    installed: true,
    downloading: false,
  },
];
function bridge(): VoiceBridge {
  return {
    models: vi.fn(async () => models),
    download: vi.fn(async () => {}),
    delete: vi.fn(async () => {}),
    inputs: vi.fn(async () => ["Mac microphone"]),
    start: vi.fn(async () => {}),
    stop: vi.fn(async () => "text"),
    cancel: vi.fn(async () => {}),
    unloadAfter: vi.fn(async () => {}),
    listen: vi.fn(async () => () => {}),
  };
}
function context(voiceBridge?: VoiceBridge): SettingsContext {
  return {
    bridge: { voice: voiceBridge } as NativeBridge,
    remote: false,
    account: {} as SettingsContext["account"],
    updateHint: false,
    githubOffered: false,
    hostSettings: false,
    instructions: false,
    gitSettings: false,
    promptSettings: false,
    repositorySettings: false,
    advancedSettings: false,
    browser: false,
    steer: false,
    attachments: false,
    autoMerge: false,
  } as SettingsContext;
}
function panel(b: VoiceBridge) {
  const ctx = context(b);
  return render(
    <>
      {visibleSettings("voice", ctx).map((s) => (
        <div key={s.id}>{s.render(ctx, s)}</div>
      ))}
    </>,
  );
}
afterEach(cleanup);

describe("Voice settings", () => {
  it("hides the tab and its settings without the native voice bridge", () => {
    expect(visibleTabs(context()).map((t) => t.id)).not.toContain("voice");
    expect(visibleSettings("voice", context())).toEqual([]);
    expect(searchSettings("dictation", context())).toEqual([]);
    expect(visibleTabs(context(bridge())).map((t) => t.id)).toContain("voice");
    expect(registryProblems()).toEqual([]);
    expect(TABS.find((t) => t.id === "voice")?.group).toBe("Settings");
  });
  it("renders the catalog recommendation and calls Download", async () => {
    const b = bridge();
    panel(b);
    const table = await screen.findByRole("table", { name: "Speech models" });
    const row = within(table).getByText("Parakeet TDT v3").closest("tr")!;
    expect(within(row).getByText("Recommended")).toBeTruthy();
    expect(within(table).getAllByText("Recommended")).toHaveLength(1);
    expect(within(row).getByText("487 MB")).toBeTruthy();
    fireEvent.click(within(row).getByRole("button", { name: "Download" }));
    await waitFor(() => expect(b.download).toHaveBeenCalledWith("parakeet-tdt-v3"));
  });
  it("disables Language for Parakeet and enables it after selecting Whisper", async () => {
    panel(bridge());
    expect((screen.getByRole("combobox", { name: "Language" }) as HTMLSelectElement).disabled).toBe(true);
    expect(screen.getByText("Parakeet TDT v3 detects it by itself.")).toBeTruthy();
    fireEvent.click(await screen.findByRole("button", { name: "Use" }));
    expect(voice().model).toBe("whisper-tiny");
    expect((screen.getByRole("combobox", { name: "Language" }) as HTMLSelectElement).disabled).toBe(false);
  });
  it("gives every row a searchable deep link and saves microphone and vocabulary choices", async () => {
    panel(bridge());
    await screen.findByRole("option", { name: "Mac microphone" });
    fireEvent.change(screen.getByRole("combobox", { name: "Input" }), {
      target: { value: "Mac microphone" },
    });
    fireEvent.change(screen.getByRole("textbox", { name: "Word" }), {
      target: { value: "Zwiim" },
    });
    fireEvent.change(screen.getByRole("textbox", { name: "Heard as (optional)" }), { target: { value: "swim" } });
    fireEvent.click(screen.getByRole("button", { name: "Add" }));
    expect(voice()).toMatchObject({
      input: "Mac microphone",
      vocabulary: [{ word: "Zwiim", heardAs: "swim" }],
    });
    for (const s of SETTINGS.filter((s) => s.tab === "voice"))
      expect(document.querySelector(`[data-setting-id="${s.id}"]`)).toBeTruthy();
    expect(searchSettings("microphone", context(bridge())).some((hit) => hit.target.tab === "voice")).toBe(true);
  });
  it("shows download errors and progress from the shell", async () => {
    const b = bridge();
    let downloadEvent: ((p: unknown) => void) | undefined;
    b.listen = vi.fn(async (_event, cb) => {
      downloadEvent = cb as (p: unknown) => void;
      return () => {};
    });
    const { act } = await import("@testing-library/react");
    panel(b);
    await screen.findByRole("table");
    act(() =>
      downloadEvent?.({
        id: "parakeet-tdt-v3",
        downloaded: 10e6,
        total: 487e6,
      }),
    );
    expect(screen.getByRole("progressbar", { name: "Downloading Parakeet TDT v3" })).toBeTruthy();
    act(() =>
      downloadEvent?.({
        id: "parakeet-tdt-v3",
        error: "Download interrupted. Try again.",
      }),
    );
    expect(screen.getByRole("alert").textContent).toContain("Download interrupted");
    expect(screen.getByRole("button", { name: "Download" })).toBeTruthy();
  });
  it("shows a download started before the settings opened as in progress, not as Download", async () => {
    const b = bridge();
    b.models = vi.fn(async () => models.map((m) => (m.id === "parakeet-tdt-v3" ? { ...m, downloading: true } : m)));
    panel(b);
    const table = await screen.findByRole("table", { name: "Speech models" });
    const row = within(table).getByText("Parakeet TDT v3").closest("tr")!;
    expect(within(row).getByRole("progressbar", { name: "Downloading Parakeet TDT v3" })).toBeTruthy();
    expect(within(row).queryByRole("button", { name: "Download" })).toBeNull();
  });
  it("subscribes through voice commands while core event permissions stay disabled", async () => {
    const b = tauriBridge().voice!;
    const listener = vi.fn();
    const off = await b.listen("partial", listener);
    expect(native.invoke).toHaveBeenCalledWith("voice_listen", { event: "partial", onEvent: native.channels.at(-1) });
    native.channels.at(-1)?.onmessage?.({ text: "local words" });
    expect(listener).toHaveBeenCalledWith({ text: "local words" });
    off();
    expect(native.invoke).toHaveBeenCalledWith("voice_unlisten", { id: 42 });
  });
});
