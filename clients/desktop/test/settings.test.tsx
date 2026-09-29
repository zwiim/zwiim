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
