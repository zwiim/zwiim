import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CLIENT_CAPABILITIES, HostClient, HostController, ZwiimClientError } from "@zwiim/sdk";
import { Workbench } from "@/App";
import type { NativeBridge } from "@/lib/bridge";
import { HostProvider } from "@/lib/host-context";
import { rememberWelcomed } from "@/lib/prefs";
import { FakeTransport, hostHello, tick } from "./fakes";
import { openMenu, openSettings } from "./ui";

afterEach(cleanup);
beforeEach(() => {
  window.localStorage.clear();
  rememberWelcomed();
});

function bridge(): NativeBridge {
  return {
    discover: vi.fn(async () => null),
    startHost: vi.fn(async () => {}),
    pickFolder: vi.fn(async () => null),
    accountStatus: vi.fn(async () => ({ available: true, account: null })),
    signIn: vi.fn(async () => ({ user_id: "u", email: null })),
    cancelSignIn: vi.fn(async () => {}),
    signOut: vi.fn(async () => {}),
  };
}

type Value = string | boolean | null;

/** A Host keeping `values`; `settings.update` merges into them, refusing a prefix git wouldn't take. */
function host(values: Record<string, Value> = {}) {
  const t = new FakeTransport();
  const kept = { ...values };
  t.handlers["settings.get"] = () => ({ values: { ...kept } });
  t.handlers["settings.update"] = (p: { values: Record<string, Value> }) => {
    const custom = p.values["git.branch_prefix_custom"];
    if (typeof custom === "string" && custom.includes(" ")) {
      throw new ZwiimClientError("invalid_input", "Not a valid value for `git.branch_prefix_custom`", 400);
    }
    Object.assign(kept, p.values);
    return { values: { ...kept } };
  };
  return t;
}

async function mount(t: FakeTransport) {
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

const updates = (t: FakeTransport) => t.calls.filter((c) => c.op === "settings.update").map((c) => c.params.values);

describe("git settings", () => {
  it("picks where branch names' prefix comes from and saves custom text on Enter", async () => {
    const t = host();
    const c = await mount(t);
    const dialog = openSettings("Git");
    await tick();
    const prefix = dialog.getByLabelText("Branch name prefix") as HTMLSelectElement;
    expect(prefix.value).toBe("");
    expect(dialog.getByText(/New branches look like pune\./)).toBeTruthy();
    expect(dialog.queryByLabelText("Custom prefix")).toBeNull();

    fireEvent.change(prefix, { target: { value: "custom" } });
    await tick();
    const custom = dialog.getByLabelText("Custom prefix");
    fireEvent.change(custom, { target: { value: " team/ashish " } });
    fireEvent.keyDown(custom, { key: "Enter" });
    await tick();
    // Leaving the field with nothing new saves nothing again.
    fireEvent.blur(custom);
    await tick();
    expect(updates(t)).toEqual([{ "git.branch_prefix": "custom" }, { "git.branch_prefix_custom": "team/ashish" }]);
    expect(dialog.getByText(/New branches look like team\/ashish\/pune\./)).toBeTruthy();

    fireEvent.change(prefix, { target: { value: "" } });
    await tick();
    expect(updates(t).at(-1)).toEqual({ "git.branch_prefix": null });
    c.dispose();
  });

  it("shows why the host refused a prefix", async () => {
    const t = host({ "git.branch_prefix": "custom" });
    const c = await mount(t);
    const dialog = openSettings("Git");
    await tick();
    const custom = dialog.getByLabelText("Custom prefix");
    fireEvent.change(custom, { target: { value: "my team" } });
    fireEvent.blur(custom);
    await tick();
    expect(dialog.getByRole("alert").textContent).toContain("git.branch_prefix_custom");
    c.dispose();
  });

  it("turns deleting the branch on remove on and off", async () => {
    const t = host({ "git.branch_prefix": "github" });
    const c = await mount(t);
    const dialog = openSettings("Git");
    await tick();
    expect(dialog.getByText(/your-username\/pune/)).toBeTruthy();
    const toggle = dialog.getByTestId("delete-branch-on-remove") as HTMLInputElement;
    expect(toggle.checked).toBe(false);
    fireEvent.click(toggle);
    await tick();
    expect(updates(t)).toEqual([{ "git.delete_branch_on_remove": true }]);
    expect((dialog.getByTestId("delete-branch-on-remove") as HTMLInputElement).checked).toBe(true);
    c.dispose();
  });

  it("is not offered by a host without Git settings", async () => {
    const t = host();
    t.handlers["host.hello"] = () =>
      hostHello({ capabilities: CLIENT_CAPABILITIES.filter((x) => x !== "settings.git") });
    const c = await mount(t);
    fireEvent.click(openMenu().getByRole("menuitem", { name: /^Settings/ }));
    const nav = within(screen.getByRole("navigation", { name: "Settings sections" }));
    expect(nav.queryByText("Git")).toBeNull();
    expect(nav.getByText("Models")).toBeTruthy();
    c.dispose();
  });
});
