import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { HostClient, HostController } from "@zwiim/sdk";
import { Workbench } from "@/App";
import type { NativeBridge } from "@/lib/bridge";
import { HostProvider } from "@/lib/host-context";
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

async function mount(t = new FakeTransport()) {
  const client = new HostClient({ transport: t, random: () => 0.5 });
  const controller = new HostController(client);
  client.start();
  await tick();
  render(
    <HostProvider value={controller}>
      <Workbench bridge={bridge()} />
    </HostProvider>,
  );
  await tick();
  return controller;
}

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

describe("theme", () => {
  it("follows the choice made in General and remembers it", async () => {
    const c = await mount();
    const dialog = openSettings("General");
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
    const group = dialog.getByRole("radiogroup", { name: "Keep this Mac reachable" });
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
    expect(screen.queryByRole("radiogroup", { name: "Keep this Mac reachable" })).toBeNull();
    c.dispose();
  });
});
