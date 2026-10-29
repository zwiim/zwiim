import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { HostClient, HostController } from "@zwiim/sdk";
import { Workbench } from "@/App";
import type { NativeBridge } from "@/lib/bridge";
import { HostProvider } from "@/lib/host-context";
import { deviceNoun } from "@/lib/platform";
import { rememberWelcomed, theme } from "@/lib/prefs";
import { FakeTransport, tick } from "./fakes";
import { closeMenu, openMenu, openSettings } from "./ui";

afterEach(cleanup);
beforeEach(() => {
  window.localStorage.clear();
  rememberWelcomed();
  document.documentElement.classList.remove("dark");
});

function bridge(): NativeBridge {
  return {
    discover: vi.fn(async () => null),
    startHost: vi.fn(async () => {}),
    pickFolder: vi.fn(async () => null),
    accountStatus: vi.fn(async () => ({ available: true, account: null })),
    signIn: vi.fn(async () => ({ user_id: "u-me", email: "me@example.com" })),
    cancelSignIn: vi.fn(async () => {}),
    signOut: vi.fn(async () => {}),
  };
}

async function mount(t = new FakeTransport(), native: NativeBridge = bridge()) {
  const client = new HostClient({ transport: t, random: () => 0.5 });
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

const KEEP_REACHABLE = `Keep ${deviceNoun()} reachable`;

const section = () => screen.getByRole("dialog", { name: "Settings" }).querySelector("h2")!.textContent;

describe("control center", () => {
  it("keeps settings in a menu at the foot of the sidebar, with the workspaces above", async () => {
    const c = await mount();
    const sidebar = screen.getByTestId("sidebar");
    expect(sidebar.textContent).toContain("Workspaces");
    expect(screen.queryByTestId("account")).toBeNull();
    const menu = openMenu();
    expect(menu.getByText("Not signed in")).toBeTruthy();
    expect(menu.getByRole("menuitem", { name: /^Settings/ })).toBeTruthy();
    expect(menu.getByText("Devices")).toBeTruthy();
    expect(menu.getByText("Sign in")).toBeTruthy();
    closeMenu();
    expect(screen.queryByRole("menu")).toBeNull();
    c.dispose();
  });

  it("opens settings at the section a menu item names, and closes on Escape", async () => {
    const c = await mount();
    fireEvent.click(openMenu().getByText("About & updates"));
    expect(screen.queryByRole("menu")).toBeNull();
    expect(section()).toBe("About & updates");
    expect(screen.getByTestId("about-desktop")).toBeTruthy();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("dialog", { name: "Settings" })).toBeNull();
    c.dispose();
  });

  it("opens settings with ⌘,", async () => {
    const c = await mount();
    fireEvent.keyDown(window, { key: ",", metaKey: true });
    expect(section()).toBe("General");
    c.dispose();
  });

  it("offers devices for this computer's host and no connection mode", async () => {
    const c = await mount();
    const dialog = openSettings("Devices");
    expect(section()).toBe("Devices");
    expect(dialog.queryByText("Connection")).toBeNull();
    c.dispose();
  });

  it("marks an update staged on the host", async () => {
    const t = new FakeTransport();
    t.handlers["host.update.status"] = () => ({ restart_required: true });
    const c = await mount(t);
    expect(screen.getByTitle("Account and settings").querySelector("[aria-label='Update available']")).toBeTruthy();
    c.dispose();
  });
});

describe("checking for updates", () => {
  it("checks the host from the menu and says when it is up to date", async () => {
    const t = new FakeTransport();
    t.handlers["host.update.status"] = () => ({ restart_required: false });
    t.handlers["host.update.check"] = () => ({ restart_required: false });
    const c = await mount(t);
    fireEvent.click(openMenu().getByRole("menuitem", { name: "Check for updates" }));
    await tick();
    expect(t.calls.some((x) => x.op === "host.update.check")).toBe(true);
    expect(screen.getByRole("menu").textContent).toContain("Up to date");
    expect(screen.queryByTestId("update-card")).toBeNull();
    closeMenu();
    expect(openMenu().getByRole("menuitem", { name: "Check for updates" })).toBeTruthy();
    c.dispose();
  });

  it("shows a found update in the card, again even after it was dismissed", async () => {
    const t = new FakeTransport();
    const staged = { restart_required: true, staged: { version: "0.4.0", source: "download" } };
    t.handlers["host.update.status"] = () => staged;
    t.handlers["host.update.check"] = () => staged;
    const c = await mount(t);
    expect(screen.getByTestId("update-card").textContent).toContain("Zwiim Host 0.4.0");
    fireEvent.click(screen.getByLabelText("Dismiss"));
    expect(screen.queryByTestId("update-card")).toBeNull();
    fireEvent.click(openMenu().getByRole("menuitem", { name: "Check for updates" }));
    await tick();
    expect(screen.queryByRole("menu")).toBeNull();
    expect(screen.getByTestId("update-card").textContent).toContain("New update available");
    c.dispose();
  });
});

describe("searching settings", () => {
  const search = () => screen.getByRole("textbox", { name: "Search settings" }) as HTMLInputElement;
  const results = () => screen.queryAllByRole("option").map((o) => o.textContent);

  it("focuses the search on Mod+F and opens a setting picked with Enter, marking it", async () => {
    const c = await mount();
    const dialog = openSettings("Account");
    fireEvent.keyDown(dialog.getByRole("navigation", { name: "Settings sections" }), { key: "f", ctrlKey: true });
    expect(document.activeElement).toBe(search());
    fireEvent.change(search(), { target: { value: "light" } });
    expect(results()[0]).toBe("ThemeAppearance");
    fireEvent.keyDown(search(), { key: "Enter" });
    expect(section()).toBe("Appearance");
    expect(search().value).toBe("");
    expect(document.querySelector('[data-setting-id="theme"]')!.hasAttribute("data-flash")).toBe(true);
    c.dispose();
  });

  it("moves through results with the arrows, wrapping at the ends", async () => {
    const c = await mount();
    openSettings("General");
    fireEvent.change(search(), { target: { value: "sign in" } });
    const options = () => screen.getAllByRole("option");
    expect(options().length).toBeGreaterThan(1);
    expect(options()[0]!.getAttribute("aria-selected")).toBe("true");
    fireEvent.keyDown(search(), { key: "ArrowUp" });
    expect(options().at(-1)!.getAttribute("aria-selected")).toBe("true");
    fireEvent.keyDown(search(), { key: "ArrowDown" });
    expect(options()[0]!.getAttribute("aria-selected")).toBe("true");
    fireEvent.keyDown(search(), { key: "ArrowDown" });
    expect(options()[1]!.getAttribute("aria-selected")).toBe("true");
    c.dispose();
  });

  it("finds a tab by its keywords", async () => {
    const c = await mount();
    openSettings("General");
    fireEvent.change(search(), { target: { value: "codex" } });
    fireEvent.mouseDown(screen.getByRole("option", { name: "Agents" }));
    expect(section()).toBe("Agents");
    c.dispose();
  });

  it("says when nothing matches", async () => {
    const c = await mount();
    openSettings("General");
    fireEvent.change(search(), { target: { value: "zzqqxx" } });
    expect(screen.getByText("No settings match.")).toBeTruthy();
    c.dispose();
  });

  it("clears the query on Escape before closing", async () => {
    const c = await mount();
    openSettings("General");
    fireEvent.change(search(), { target: { value: "dark" } });
    fireEvent.keyDown(search(), { key: "Escape" });
    expect(search().value).toBe("");
    expect(screen.getByRole("dialog", { name: "Settings" })).toBeTruthy();
    fireEvent.keyDown(search(), { key: "Escape" });
    expect(screen.queryByRole("dialog", { name: "Settings" })).toBeNull();
    c.dispose();
  });
});

describe("theme", () => {
  it("follows the choice made in Appearance and remembers it", async () => {
    const c = await mount();
    const dialog = openSettings("Appearance");
    fireEvent.click(dialog.getByRole("radio", { name: "Dark" }));
    expect(document.documentElement.classList.contains("dark")).toBe(true);
    expect(theme()).toBe("dark");
    fireEvent.click(dialog.getByRole("radio", { name: "Light" }));
    expect(document.documentElement.classList.contains("dark")).toBe(false);
    expect(dialog.getByRole("radio", { name: "Light" }).getAttribute("aria-checked")).toBe("true");
    c.dispose();
  });
});

type Reach = {
  supported: boolean;
  mode: "off" | "while_working" | "always";
  on_battery: boolean;
  lid_closed: { support: "no_lid" | "unavailable" | "needs_setup" | "available"; enabled: boolean };
  active: boolean;
  blocked_by: "on_battery" | "low_battery" | "thermal" | null;
  warnings: ("lid_close_will_sleep" | "on_battery_will_drain")[];
  power: { source: "ac" | "battery" | "unknown"; battery_percent: number | null };
};

function status(over: Partial<Reach> = {}): Reach {
  return {
    supported: true,
    mode: "always",
    on_battery: false,
    lid_closed: { support: "unavailable", enabled: false },
    active: true,
    blocked_by: null,
    warnings: [],
    power: { source: "ac", battery_percent: 100 },
    ...over,
  };
}

describe("keeping the Mac reachable", () => {
  it("offers the reachability mode in General and sends only what changed", async () => {
    const t = new FakeTransport();
    let current = status();
    t.handlers["host.keep_awake.status"] = () => current;
    t.handlers["host.keep_awake.set"] = (p) => {
      current = { ...current, ...(p as Partial<Reach>) };
      return current;
    };
    const c = await mount(t);
    const dialog = openSettings("General");
    await tick();
    const group = dialog.getByRole("radiogroup", { name: KEEP_REACHABLE });
    expect(group).toBeTruthy();
    expect(dialog.getByRole("radio", { name: "Always (recommended)" }).getAttribute("aria-checked")).toBe("true");
    fireEvent.click(dialog.getByRole("radio", { name: "Only while agents work" }));
    await tick();
    expect(dialog.getByRole("radio", { name: "Only while agents work" }).getAttribute("aria-checked")).toBe("true");
    const battery = screen.getByTestId("keep-awake-battery") as HTMLInputElement;
    expect(battery.checked).toBe(false);
    fireEvent.click(battery);
    await tick();
    fireEvent.click(dialog.getByRole("radio", { name: "Never" }));
    await tick();
    expect((screen.getByTestId("keep-awake-battery") as HTMLInputElement).disabled).toBe(true);
    expect(t.calls.filter((x) => x.op === "host.keep_awake.set").map((x) => x.params)).toEqual([
      { mode: "while_working" },
      { on_battery: true },
      { mode: "off" },
    ]);
    expect(screen.getByTestId("keep-awake-status").textContent).toContain("This Mac sleeps as usual");
    c.dispose();
  });

  it("says why the Mac will sleep", async () => {
    const cases: [Partial<Reach>, string][] = [
      [{ mode: "off", active: false }, "This Mac sleeps as usual"],
      [{ active: false, blocked_by: "on_battery" }, "Will sleep: on battery"],
      [{ active: false, blocked_by: "low_battery" }, "Will sleep: battery below 10%"],
      // The host checks the 10% floor first; so does the copy.
      [
        { active: false, blocked_by: "low_battery", power: { source: "battery", battery_percent: 5 } },
        "Will sleep: battery below 10%",
      ],
      [{ active: false, blocked_by: "thermal" }, "Will sleep: Mac is too hot"],
      [{ active: true }, "Reachable: holding awake"],
      [{ mode: "while_working", active: false }, "Reachable while agents work"],
      [{ mode: "always", active: false }, "Could not keep this Mac awake"],
      // While working, nothing is blocked until a turn starts; say so before.
      [
        { mode: "while_working", active: false, power: { source: "battery", battery_percent: 80 } },
        "Will sleep: on battery",
      ],
      [
        {
          mode: "while_working",
          active: false,
          on_battery: true,
          power: { source: "battery", battery_percent: 9 },
        },
        "Will sleep: battery below 10%",
      ],
    ];
    for (const [over, text] of cases) {
      const t = new FakeTransport();
      t.handlers["host.keep_awake.status"] = () => status(over);
      const c = await mount(t);
      openSettings("General");
      await tick();
      expect(screen.getByTestId("keep-awake-status").textContent).toContain(text);
      c.dispose();
      cleanup();
    }
    const t = new FakeTransport();
    t.handlers["host.keep_awake.status"] = () =>
      status({ warnings: ["lid_close_will_sleep", "on_battery_will_drain"] });
    const c = await mount(t);
    const dialog = openSettings("General");
    await tick();
    expect(dialog.getByText("Closing the lid will still sleep this Mac")).toBeTruthy();
    expect(dialog.getByText("Running on battery: this drains it")).toBeTruthy();
    expect(dialog.getByText("The screen still turns off. Lock your Mac, don't log out.")).toBeTruthy();
    c.dispose();
  });

  it("shows why a change was refused", async () => {
    const t = new FakeTransport();
    t.handlers["host.keep_awake.status"] = () => status();
    t.handlers["host.keep_awake.set"] = () => {
      throw new Error("nope");
    };
    const c = await mount(t);
    const dialog = openSettings("General");
    await tick();
    fireEvent.click(dialog.getByRole("radio", { name: "Never" }));
    await tick();
    expect(dialog.getByRole("alert")).toBeTruthy();
    c.dispose();
  });

  it("follows the host feed while open", async () => {
    const t = new FakeTransport();
    t.handlers["host.keep_awake.status"] = () => status({ active: true });
    const c = await mount(t);
    openSettings("General");
    await tick();
    expect(screen.getByTestId("keep-awake-status").textContent).toContain("Reachable: holding awake");
    await t.feedEvent("host.keep_awake_changed", status({ active: false, blocked_by: "on_battery" }));
    expect(screen.getByTestId("keep-awake-status").textContent).toContain("Will sleep: on battery");
    expect(t.calls.filter((x) => x.op === "host.keep_awake.status")).toHaveLength(1);
    c.dispose();
  });

  it("a change made here shows at once, then follows the feed", async () => {
    const t = new FakeTransport();
    let current = status();
    t.handlers["host.keep_awake.status"] = () => current;
    t.handlers["host.keep_awake.set"] = (p) => {
      current = { ...current, ...(p as Partial<Reach>), active: false };
      return current;
    };
    const c = await mount(t);
    const dialog = openSettings("General");
    await tick();
    fireEvent.click(dialog.getByRole("radio", { name: "Never" }));
    await tick();
    expect(screen.getByTestId("keep-awake-status").textContent).toContain("This Mac sleeps as usual");
    await t.feedEvent("host.keep_awake_changed", status({ mode: "always", active: true }));
    expect(screen.getByTestId("keep-awake-status").textContent).toContain("Reachable: holding awake");
    c.dispose();
  });

  it("is not offered where the host cannot keep the computer awake", async () => {
    const t = new FakeTransport();
    t.handlers["host.keep_awake.status"] = () => status({ supported: false });
    const c = await mount(t);
    openSettings("General");
    await tick();
    expect(screen.queryByRole("radiogroup", { name: KEEP_REACHABLE })).toBeNull();
    c.dispose();
  });
});

describe("staying awake with the lid closed", () => {
  const lid = (support: Reach["lid_closed"]["support"], enabled = false) => status({ lid_closed: { support, enabled } });

  it("lid row hidden without a lid or helper", async () => {
    for (const support of ["no_lid", "unavailable"] as const) {
      const t = new FakeTransport();
      t.handlers["host.keep_awake.status"] = () => lid(support);
      const c = await mount(t);
      openSettings("General");
      await tick();
      expect(screen.getByTestId("keep-awake-status")).toBeTruthy();
      expect(screen.queryByTestId("keep-awake-lid")).toBeNull();
      c.dispose();
      cleanup();
    }
  });

  it("available lid toggles lid_closed", async () => {
    const t = new FakeTransport();
    let current = lid("available");
    t.handlers["host.keep_awake.status"] = () => current;
    t.handlers["host.keep_awake.set"] = (p) => {
      const { lid_closed } = p as { lid_closed: boolean };
      current = { ...current, lid_closed: { ...current.lid_closed, enabled: lid_closed } };
      return current;
    };
    const c = await mount(t);
    const dialog = openSettings("General");
    await tick();
    const row = screen.getByTestId("keep-awake-lid");
    const box = dialog.getByRole("checkbox", { name: "Stay awake with the lid closed" }) as HTMLInputElement;
    expect(box.checked).toBe(false);
    expect(row.textContent).toContain("Don't put the Mac in a bag while this is on.");
    fireEvent.click(box);
    await tick();
    expect(t.calls.filter((x) => x.op === "host.keep_awake.set").map((x) => x.params)).toEqual([{ lid_closed: true }]);
    expect((dialog.getByRole("checkbox", { name: "Stay awake with the lid closed" }) as HTMLInputElement).checked).toBe(
      true,
    );
    c.dispose();
  });

  it("needs_setup shows Set up", async () => {
    const t = new FakeTransport();
    t.handlers["host.keep_awake.status"] = () => lid("needs_setup");
    const setup = vi.fn(async () => "requires_approval" as const);
    const c = await mount(t, { ...bridge(), lidHelper: { setup } });
    const dialog = openSettings("General");
    await tick();
    expect(screen.getByTestId("keep-awake-lid")).toBeTruthy();
    expect(dialog.queryByRole("checkbox", { name: "Stay awake with the lid closed" })).toBeNull();
    const reads = () => t.calls.filter((x) => x.op === "host.keep_awake.status").length;
    const before = reads();
    fireEvent.click(dialog.getByRole("button", { name: "Set up" }));
    await tick();
    await tick();
    expect(setup).toHaveBeenCalledTimes(1);
    expect(reads()).toBe(before + 1);
    expect(screen.getByTestId("keep-awake-lid").textContent).toContain(
      "Allow Zwiim in System Settings → Login Items",
    );
    c.dispose();
  });

  it("offers no Set up without a way to install the helper", async () => {
    const t = new FakeTransport();
    t.handlers["host.keep_awake.status"] = () => lid("needs_setup");
    const c = await mount(t);
    const dialog = openSettings("General");
    await tick();
    expect(dialog.queryByRole("button", { name: "Set up" })).toBeNull();
    c.dispose();
  });
});
