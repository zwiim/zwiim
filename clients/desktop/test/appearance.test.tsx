import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { HostClient, HostController } from "@zwiim/sdk";
import { Workbench } from "@/App";
import {
  fontChoices,
  MAX_OTHER_FONTS,
  MONO_FONTS,
  monoFonts,
  stepZoom,
  terminalFonts,
  watchAppearance,
} from "@/lib/appearance";
import type { NativeBridge } from "@/lib/bridge";
import { HostProvider } from "@/lib/host-context";
import { platform } from "@/lib/platform";
import {
  codeLigatures,
  colorVision,
  fonts,
  markdownStyle,
  rememberCodeLigatures,
  rememberColorVision,
  rememberFonts,
  rememberSoftDark,
  rememberWelcomed,
  rememberZoom,
  softDark,
  zoom,
} from "@/lib/prefs";
import { FakeTransport, tick } from "./fakes";
import { openSettings } from "./ui";

afterEach(cleanup);
beforeEach(() => {
  window.localStorage.clear();
  rememberWelcomed();
  const root = document.documentElement;
  root.style.removeProperty("--font-mono");
  root.classList.remove("no-code-ligatures", "soft-dark");
  root.removeAttribute("data-color-vision");
});

function bridge(extra: Partial<NativeBridge> = {}): NativeBridge {
  return {
    discover: vi.fn(async () => null),
    startHost: vi.fn(async () => {}),
    pickFolder: vi.fn(async () => null),
    accountStatus: vi.fn(async () => ({ available: true, account: null })),
    signIn: vi.fn(async () => ({ user_id: "u-me", email: "me@example.com" })),
    cancelSignIn: vi.fn(async () => {}),
    signOut: vi.fn(async () => {}),
    ...extra,
  };
}

async function mount(native: NativeBridge) {
  const client = new HostClient({ transport: new FakeTransport(), random: () => 0.5 });
  const controller = new HostController(client);
  client.start();
  await tick();
  render(
    <HostProvider value={controller}>
      <Workbench bridge={native} />
    </HostProvider>,
  );
  await tick();
  return controller;
}

const mod = platform() === "macos" ? { metaKey: true } : { ctrlKey: true };

describe("appearance preferences", () => {
  it("keeps only offered zoom levels and steps between them, stopping at either end", () => {
    expect(zoom()).toBe(1);
    rememberZoom(1.17);
    expect(zoom()).toBe(1);
    expect(stepZoom(1, 1)).toBe(1.1);
    expect(stepZoom(1, -1)).toBe(0.9);
    expect(stepZoom(1.5, 1)).toBe(1.5);
    expect(stepZoom(0.75, -1)).toBe(0.75);
    expect(stepZoom(1.17, 1)).toBe(1.1);
  });

  it("keeps font names one CSS string can hold, and a terminal size in range", () => {
    expect(fonts()).toEqual({ code: "", terminal: "", terminalSize: 12 });
    rememberFonts({ code: ' Fira "Code"\\\n', terminalSize: 99 });
    expect(fonts()).toEqual({ code: "Fira Code", terminal: "", terminalSize: 24 });
    rememberFonts({ terminalSize: 3.4 });
    expect(fonts().terminalSize).toBe(9);
  });

  it("draws the terminal in its own font, else the code font, else the app's", () => {
    expect(terminalFonts()).toBe(MONO_FONTS);
    rememberFonts({ code: "Fira Code" });
    expect(terminalFonts()).toBe(monoFonts("Fira Code"));
    rememberFonts({ terminal: "Menlo" });
    expect(terminalFonts()).toBe(`"Menlo", ${MONO_FONTS}`);
  });

  it("offers monospace fonts, and the rest only once searched for", () => {
    const all = [
      { name: "Arial", mono: false },
      { name: "Fira Code", mono: true },
      { name: "Menlo", mono: true },
    ];
    expect(fontChoices(all, "")).toEqual({ mono: ["Fira Code", "Menlo"], other: [] });
    expect(fontChoices(all, " AR ")).toEqual({ mono: [], other: ["Arial"] });
    expect(fontChoices(all, "e")).toEqual({ mono: ["Fira Code", "Menlo"], other: [] });
    const many = Array.from({ length: 80 }, (_, i) => ({ name: `Sans ${i}`, mono: false }));
    expect(fontChoices(many, "sans").other).toHaveLength(MAX_OTHER_FONTS);
  });

  it("applies zoom and the code font to the window only when they change", () => {
    const set = vi.fn(async () => {});
    const stop = watchAppearance(bridge({ zoom: { set } }));
    expect(set).not.toHaveBeenCalled();
    rememberZoom(1.25);
    rememberFonts({ terminalSize: 14 });
    expect(set.mock.calls).toEqual([[1.25]]);

    rememberFonts({ code: "Fira Code" });
    expect(document.documentElement.style.getPropertyValue("--font-mono")).toBe(`"Fira Code", ${MONO_FONTS}`);
    rememberFonts({ code: "" });
    expect(document.documentElement.style.getPropertyValue("--font-mono")).toBe("");
    stop();
  });

  it("draws ligatures and the usual colors until told otherwise, keeping only known choices", () => {
    expect([codeLigatures(), softDark(), colorVision(), markdownStyle()]).toEqual([true, false, "default", "default"]);
    window.localStorage.setItem("zwiim.desktop.color_vision", JSON.stringify("purple"));
    window.localStorage.setItem("zwiim.desktop.markdown_style", JSON.stringify("fancy"));
    window.localStorage.setItem("zwiim.desktop.soft_dark", JSON.stringify("yes"));
    expect([softDark(), colorVision(), markdownStyle()]).toEqual([false, "default", "default"]);
  });

  it("marks the window for ligatures off, softer dark and color vision, and unmarks it", () => {
    const root = document.documentElement;
    const stop = watchAppearance(bridge());
    expect(root.classList.contains("no-code-ligatures")).toBe(false);
    expect(root.classList.contains("soft-dark")).toBe(false);
    expect(root.hasAttribute("data-color-vision")).toBe(false);

    rememberCodeLigatures(false);
    rememberSoftDark(true);
    rememberColorVision("red_green");
    expect(root.classList.contains("no-code-ligatures")).toBe(true);
    expect(root.classList.contains("soft-dark")).toBe(true);
    expect(root.getAttribute("data-color-vision")).toBe("red_green");

    rememberColorVision("blue_yellow");
    expect(root.getAttribute("data-color-vision")).toBe("blue_yellow");
    rememberCodeLigatures(true);
    rememberSoftDark(false);
    rememberColorVision("default");
    expect(root.classList.contains("no-code-ligatures")).toBe(false);
    expect(root.classList.contains("soft-dark")).toBe(false);
    expect(root.hasAttribute("data-color-vision")).toBe(false);
    stop();
  });
});

describe("Appearance settings", () => {
  it("zooms from its stepper and from Mod with = - 0", async () => {
    const c = await mount(bridge({ zoom: { set: vi.fn(async () => {}) } }));
    const dialog = openSettings("Appearance");
    fireEvent.click(dialog.getByRole("button", { name: "Larger zoom" }));
    expect(zoom()).toBe(1.1);
    expect(dialog.getByTitle("Reset zoom").textContent).toBe("110%");
    fireEvent.click(dialog.getByTitle("Reset zoom"));
    expect(zoom()).toBe(1);

    fireEvent.keyDown(window, { key: "-", ...mod });
    fireEvent.keyDown(window, { key: "-", ...mod });
    expect(zoom()).toBe(0.8);
    fireEvent.keyDown(window, { key: "=", ...mod });
    expect(zoom()).toBe(0.9);
    fireEvent.keyDown(window, { key: "0", ...mod });
    expect(zoom()).toBe(1);
    fireEvent.keyDown(window, { key: "=" });
    expect(zoom()).toBe(1);
    c.dispose();
  });

  it("offers no zoom where the window cannot zoom", async () => {
    const c = await mount(bridge());
    const dialog = openSettings("Appearance");
    expect(dialog.getByText("Theme")).toBeTruthy();
    expect(dialog.queryByText("Zoom")).toBeNull();
    fireEvent.keyDown(window, { key: "=", ...mod });
    expect(zoom()).toBe(1);
    c.dispose();
  });

  it("picks fonts from those installed, or by a typed name, and a terminal size from its stepper", async () => {
    const list = vi.fn(async () => [
      { name: "Arial", mono: false },
      { name: "Fira Code", mono: true },
      { name: "Menlo", mono: true },
    ]);
    const c = await mount(bridge({ fonts: { list } }));
    const dialog = openSettings("Appearance");
    const options = () => within(dialog.getByRole("listbox")).getAllByRole("option").map((o) => o.textContent);

    fireEvent.click(dialog.getByRole("button", { name: "Code font: Default" }));
    await tick();
    expect(options()).toEqual(["DefaultJetBrains Mono", "Fira Code", "Menlo"]);
    const search = dialog.getByRole("textbox", { name: "Search fonts" });
    fireEvent.change(search, { target: { value: "ar" } });
    expect(options()).toEqual(["Arial", "Use “ar”"]);
    expect(dialog.getByText("Not monospace")).toBeTruthy();
    fireEvent.change(search, { target: { value: "fira" } });
    fireEvent.keyDown(search, { key: "Enter" });
    expect(fonts().code).toBe("Fira Code");
    expect(dialog.queryByRole("listbox")).toBeNull();

    const terminal = dialog.getByRole("button", { name: "Terminal font: Default" });
    expect(terminal.textContent).toBe("Default Fira Code");
    fireEvent.click(terminal);
    await tick();
    fireEvent.change(dialog.getByRole("textbox", { name: "Search fonts" }), { target: { value: ' My "Font" ' } });
    fireEvent.keyDown(dialog.getByRole("textbox", { name: "Search fonts" }), { key: "Enter" });
    expect(fonts().terminal).toBe("My Font");

    fireEvent.click(dialog.getByRole("button", { name: "Code font: Fira Code" }));
    await tick();
    fireEvent.keyDown(dialog.getByRole("textbox", { name: "Search fonts" }), { key: "Escape" });
    expect(dialog.queryByRole("listbox")).toBeNull();
    expect(screen.getByRole("dialog")).toBeTruthy();
    fireEvent.click(dialog.getByRole("button", { name: "Code font: Fira Code" }));
    await tick();
    fireEvent.click(within(dialog.getByRole("listbox")).getByRole("option", { name: /Default/ }));
    expect(fonts().code).toBe("");
    expect(list).toHaveBeenCalledTimes(1);

    fireEvent.click(dialog.getByRole("button", { name: "Larger terminal text size" }));
    expect(fonts().terminalSize).toBe(13);
    expect(dialog.getByText("13px")).toBeTruthy();
    fireEvent.click(dialog.getByText("13px"));
    expect(fonts().terminalSize).toBe(12);
    c.dispose();
  });

  it("previews code and the terminal in the font the list is on, and the terminal at its size", async () => {
    const list = vi.fn(async () => [
      { name: "Fira Code", mono: true },
      { name: "Menlo", mono: true },
    ]);
    const c = await mount(bridge({ fonts: { list } }));
    const dialog = openSettings("Appearance");
    const code = () => dialog.getByTestId("code-font-preview").style.fontFamily;
    const terminal = () => dialog.getByTestId("terminal-font-preview").style;
    const fontsOf = (name: string) => monoFonts(name).replace(/"/g, "");
    const norm = (f: string) => f.replace(/"/g, "");
    expect(norm(code())).toBe(fontsOf(""));
    expect(terminal().fontSize).toBe("12px");

    fireEvent.click(dialog.getByRole("button", { name: "Code font: Default" }));
    await tick();
    const search = dialog.getByRole("textbox", { name: "Search fonts" });
    fireEvent.keyDown(search, { key: "ArrowDown" });
    expect(norm(code())).toBe(fontsOf("Fira Code"));
    fireEvent.pointerEnter(dialog.getByRole("option", { name: "Menlo" }));
    expect(norm(code())).toBe(fontsOf("Menlo"));
    expect(norm(terminal().fontFamily)).toBe(fontsOf(""));
    fireEvent.keyDown(search, { key: "Escape" });
    expect(norm(code())).toBe(fontsOf(""));

    rememberFonts({ code: "Fira Code", terminalSize: 15 });
    await tick();
    expect(norm(code())).toBe(fontsOf("Fira Code"));
    expect(norm(terminal().fontFamily)).toBe(fontsOf("Fira Code"));
    expect(terminal().fontSize).toBe("15px");
    c.dispose();
  });

  it("turns ligatures and the softer dark theme on and off, and picks color vision and markdown style", async () => {
    const c = await mount(bridge());
    const dialog = openSettings("Appearance");
    fireEvent.click(dialog.getByTestId("code-ligatures"));
    expect(codeLigatures()).toBe(false);
    fireEvent.click(dialog.getByTestId("soft-dark"));
    expect(softDark()).toBe(true);

    fireEvent.change(dialog.getByRole("combobox", { name: "Color vision" }), { target: { value: "red_green" } });
    expect(colorVision()).toBe("red_green");

    const preview = () => within(dialog.getByTestId("markdown-style-preview")).getByTestId("markdown");
    expect(preview().className).not.toContain("markdown-book");
    fireEvent.change(dialog.getByRole("combobox", { name: "Markdown files" }), { target: { value: "book" } });
    expect(markdownStyle()).toBe("book");
    await tick();
    expect(preview().className).toContain("markdown-book");
    c.dispose();
  });
});
