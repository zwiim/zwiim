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
});
