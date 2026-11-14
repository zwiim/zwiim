import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CLIENT_CAPABILITIES, HostClient, HostController } from "@zwiim/sdk";
import { Workbench } from "@/App";
import type { NativeBridge } from "@/lib/bridge";
import { HostProvider } from "@/lib/host-context";
import { rememberWelcomed } from "@/lib/prefs";
import { scrollbackLines } from "@/panels/TerminalPanel";
import { FakeTransport, hostHello, tick } from "./fakes";
import { openMenu, openSettings } from "./ui";

afterEach(cleanup);
beforeEach(() => {
  window.localStorage.clear();
  rememberWelcomed();
});

function bridge(extra: Partial<NativeBridge> = {}): NativeBridge {
  return {
    discover: vi.fn(async () => null),
    startHost: vi.fn(async () => {}),
    pickFolder: vi.fn(async () => null),
    accountStatus: vi.fn(async () => ({ available: true, account: null })),
    signIn: vi.fn(async () => ({ user_id: "u", email: null })),
    cancelSignIn: vi.fn(async () => {}),
    signOut: vi.fn(async () => {}),
    ...extra,
  };
}

type Value = string | number | boolean | null;

const FOLDERS = {
  data: "/Users/me/Library/Application Support/zwiim",
  worktrees: "/Users/me/Library/Application Support/zwiim/worktrees",
  repositories: "/Users/me/Library/Application Support/zwiim/repos",
};

/** A Host keeping `values`; `settings.update` merges into them. */
function host(values: Record<string, Value> = {}) {
  const t = new FakeTransport();
  const kept = { ...values };
  t.handlers["settings.get"] = () => ({ values: { ...kept } });
  t.handlers["settings.update"] = (p: { values: Record<string, Value> }) => {
    Object.assign(kept, p.values);
    return { values: { ...kept } };
  };
  t.handlers["host.folders"] = () => FOLDERS;
  return t;
}

async function mount(t: FakeTransport, b = bridge()) {
  const client = new HostClient({ transport: t, random: () => 0.5 });
  const controller = new HostController(client);
  client.start();
  await tick();
  render(
    <HostProvider value={controller}>
      <Workbench bridge={b} />
    </HostProvider>,
  );
  await tick();
  return controller;
}

const updates = (t: FakeTransport) => t.calls.filter((c) => c.op === "settings.update").map((c) => c.params.values);

describe("advanced settings", () => {
  it("picks how much terminal output the host keeps, back to its default", async () => {
    const t = host();
    const c = await mount(t);
    const dialog = openSettings("Advanced");
    await tick();
    const keep = dialog.getByLabelText("Terminal output to keep") as HTMLSelectElement;
    expect(keep.value).toBe("");
    expect([...keep.options].map((o) => o.text)).toEqual(["1 MB (default)", "256 KB", "4 MB", "16 MB"]);

    fireEvent.change(keep, { target: { value: String(4 * 1024 * 1024) } });
    await tick();
    fireEvent.change(keep, { target: { value: "" } });
    await tick();
    expect(updates(t)).toEqual([{ "terminal.scrollback": 4 * 1024 * 1024 }, { "terminal.scrollback": null }]);
    c.dispose();
  });

  it("shows an amount set elsewhere as chosen", async () => {
    const c = await mount(host({ "terminal.scrollback": 2 * 1024 * 1024 }));
    const dialog = openSettings("Advanced");
    await tick();
    const keep = dialog.getByLabelText("Terminal output to keep") as HTMLSelectElement;
    expect(keep.value).toBe(String(2 * 1024 * 1024));
    expect(keep.selectedOptions[0]?.text).toBe("2 MB");
    c.dispose();
  });

  it("sets upstream on push unless turned off", async () => {
    const t = host();
    const c = await mount(t);
    const dialog = openSettings("Advanced");
    await tick();
    const toggle = dialog.getByTestId("push-sets-upstream") as HTMLInputElement;
    expect(toggle.checked).toBe(true);
    fireEvent.click(toggle);
    await tick();
    fireEvent.click(dialog.getByTestId("push-sets-upstream"));
    await tick();
    expect(updates(t)).toEqual([{ "git.push_sets_upstream": false }, { "git.push_sets_upstream": null }]);
    c.dispose();
  });

  it("names the host's folders and shows one in the file manager", async () => {
    const reveal = vi.fn(async () => {});
    const c = await mount(host(), bridge({ folders: { reveal } }));
    const dialog = openSettings("Advanced");
    await tick();
    const folders = within(dialog.getByText("Folders").closest("section")!);
    expect(folders.getByText(FOLDERS.worktrees)).toBeTruthy();
    expect(folders.queryByText(FOLDERS.data)).toBeNull();
    expect(folders.queryByText("Data")).toBeNull();
    fireEvent.click(folders.getAllByRole("button", { name: "Show" })[0]!);
    expect(reveal).toHaveBeenCalledWith(FOLDERS.worktrees);
    c.dispose();
  });

  it("names the folders without a way to show them where this window cannot", async () => {
    const c = await mount(host());
    const dialog = openSettings("Advanced");
    await tick();
    expect(dialog.getByText(FOLDERS.repositories)).toBeTruthy();
    expect(dialog.queryByRole("button", { name: "Show" })).toBeNull();
    c.dispose();
  });

  it("offers no way to change an agent's binary", async () => {
    const c = await mount(host());
    const dialog = openSettings("Advanced");
    await tick();
    expect(dialog.queryByText(/binary|executable|path to/i)).toBeNull();
    c.dispose();
  });

  it("is not offered by a host without advanced settings", async () => {
    const t = host();
    t.handlers["host.hello"] = () =>
      hostHello({ capabilities: CLIENT_CAPABILITIES.filter((x) => x !== "settings.advanced") });
    const c = await mount(t);
    fireEvent.click(openMenu().getByRole("menuitem", { name: /^Settings/ }));
    const nav = within(screen.getByRole("navigation", { name: "Settings sections" }));
    expect(nav.queryByText("Advanced")).toBeNull();
    c.dispose();
  });
});

describe("terminal scrollback", () => {
  it("keeps about a line per 80 bytes the host keeps, within bounds", () => {
    expect(scrollbackLines(1024 * 1024)).toBe(13107);
    expect(scrollbackLines(64 * 1024)).toBe(1000);
    expect(scrollbackLines(16 * 1024 * 1024)).toBe(50_000);
  });

  it("chooses where the browser saves downloads, and goes back to the worktrees", async () => {
    const t = host();
    let root: string | null = null;
    t.handlers["browser.downloads.location"] = () => ({ root });
    t.handlers["browser.downloads.set_location"] = (p: { root: string | null }) => {
      root = p.root;
      return { root };
    };
    await mount(t, bridge({ pickFolder: vi.fn(async () => "/Users/me/Saved") }));
    const dialog = openSettings("Browser");
    await tick();
    expect(dialog.getByText("Downloads")).toBeTruthy();
    expect(dialog.getByText(/saves what pages download in its worktree, under .zwiim\/browser/)).toBeTruthy();
    fireEvent.click(dialog.getByText("Choose folder…"));
    await tick();
    expect(t.calls.find((c) => c.op === "browser.downloads.set_location")!.params).toEqual({ root: "/Users/me/Saved" });
    expect(dialog.getByText("/Users/me/Saved")).toBeTruthy();
    expect(dialog.getByText(/zwiim\/<repository>\/downloads/)).toBeTruthy();
    fireEvent.click(dialog.getByText("Use worktrees"));
    await tick();
    expect(t.calls.filter((c) => c.op === "browser.downloads.set_location").at(-1)!.params).toEqual({ root: null });
    expect(dialog.queryByText("/Users/me/Saved")).toBeNull();
  });

  it("lets agents type passwords on any site only once turned on", async () => {
    const t = host();
    const c = await mount(t);
    const dialog = openSettings("Browser");
    await tick();
    const toggle = dialog.getByTestId("browser-agent-passwords") as HTMLInputElement;
    expect(toggle.checked).toBe(false);
    expect(dialog.getByText(/only on this computer's dev servers/)).toBeTruthy();
    fireEvent.click(toggle);
    await tick();
    expect((dialog.getByTestId("browser-agent-passwords") as HTMLInputElement).checked).toBe(true);
    fireEvent.click(dialog.getByTestId("browser-agent-passwords"));
    await tick();
    expect(updates(t)).toEqual([{ "browser.agent_passwords": true }, { "browser.agent_passwords": null }]);
    c.dispose();
  });

  it("asks before agents open new sites only once turned on", async () => {
    const t = host();
    const c = await mount(t);
    const dialog = openSettings("Browser");
    await tick();
    const toggle = dialog.getByTestId("browser-agent-ask-sites") as HTMLInputElement;
    expect(toggle.checked).toBe(false);
    expect(dialog.getByText(/the page waits until you allow it/)).toBeTruthy();
    fireEvent.click(toggle);
    await tick();
    expect(updates(t)).toEqual([{ "browser.agent_ask_sites": true }]);
    c.dispose();
  });

  it("opens dev servers in the session's tab unless turned off", async () => {
    const t = host();
    const c = await mount(t);
    const dialog = openSettings("Browser");
    await tick();
    const toggle = dialog.getByTestId("browser-open-dev-servers") as HTMLInputElement;
    expect(toggle.checked).toBe(true);
    fireEvent.click(toggle);
    await tick();
    expect(updates(t)).toEqual([{ "browser.open_dev_servers": false }]);
    c.dispose();
  });

  it("holds 20 tabs per session unless another count is chosen", async () => {
    const t = host();
    const c = await mount(t);
    const dialog = openSettings("Browser");
    await tick();
    const tabs = dialog.getByLabelText("Tabs per session") as HTMLSelectElement;
    expect(tabs.value).toBe("");
    expect([...tabs.options].map((o) => o.text)).toEqual(["20 (default)", "5", "10", "30"]);
    expect(dialog.getByText(/those its pages open by themselves included/)).toBeTruthy();
    fireEvent.change(tabs, { target: { value: "10" } });
    await tick();
    fireEvent.change(dialog.getByLabelText("Tabs per session"), { target: { value: "" } });
    await tick();
    expect(updates(t)).toEqual([{ "browser.session_tabs": 10 }, { "browser.session_tabs": null }]);
    c.dispose();
  });

  it("keeps 50 MB of recording for 7 days unless chosen otherwise", async () => {
    const t = host();
    const c = await mount(t);
    const dialog = openSettings("Browser");
    await tick();
    const mb = dialog.getByLabelText("Recording per session") as HTMLSelectElement;
    expect([...mb.options].map((o) => o.text)).toEqual(["50 MB (default)", "10 MB", "200 MB", "1000 MB"]);
    const days = dialog.getByLabelText("Keep recordings for") as HTMLSelectElement;
    expect([...days.options].map((o) => o.text)).toEqual(["7 days (default)", "1 day", "30 days", "90 days"]);
    fireEvent.change(mb, { target: { value: "200" } });
    await tick();
    fireEvent.change(dialog.getByLabelText("Keep recordings for"), { target: { value: "30" } });
    await tick();
    expect(updates(t)).toEqual([{ "browser.timeline_mb": 200 }, { "browser.timeline_days": 30 }]);
    c.dispose();
  });

  it("leaves the downloads row out while the host does not say where it saves", async () => {
    await mount(host());
    const dialog = openSettings("Browser");
    await tick();
    expect(dialog.queryByText("Downloads")).toBeNull();
  });

  it("is not offered by a host without the browser", async () => {
    const t = host();
    t.handlers["host.hello"] = () => hostHello({ capabilities: CLIENT_CAPABILITIES.filter((x) => x !== "browser") });
    const c = await mount(t);
    fireEvent.click(openMenu().getByRole("menuitem", { name: /^Settings/ }));
    const nav = within(screen.getByRole("navigation", { name: "Settings sections" }));
    expect(nav.queryByText("Browser")).toBeNull();
    expect(nav.getByText("Advanced")).toBeTruthy();
    c.dispose();
  });
});
