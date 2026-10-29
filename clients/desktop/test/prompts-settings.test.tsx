import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CLIENT_CAPABILITIES, HostClient, HostController, MAX_PROMPT_BYTES } from "@zwiim/sdk";
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

/** A Host keeping `values`; `settings.update` merges into them. */
function host(values: Record<string, Value> = {}) {
  const t = new FakeTransport();
  const kept = { ...values };
  t.handlers["settings.get"] = () => ({ values: { ...kept } });
  t.handlers["settings.update"] = (p: { values: Record<string, Value> }) => {
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

describe("prompt settings", () => {
  it("shows what the host keeps and saves the user's words when leaving a field", async () => {
    const t = host({ "prompts.branch_name": "Start with the ticket number." });
    const c = await mount(t);
    const dialog = openSettings("Prompts");
    await tick();
    expect((dialog.getByLabelText("Branch names") as HTMLTextAreaElement).value).toBe("Start with the ticket number.");

    const pr = dialog.getByLabelText("Pull requests");
    fireEvent.change(pr, { target: { value: "  Open drafts.\nStart the title with the area.  " } });
    fireEvent.blur(pr);
    await tick();
    // Leaving again with nothing new saves nothing again.
    fireEvent.blur(pr);
    await tick();
    expect(updates(t)).toEqual([{ "prompts.pull_request": "Open drafts.\nStart the title with the area." }]);
    // General and conflict instructions are the Instructions tab's.
    expect(dialog.queryByLabelText("For all work")).toBeNull();
    expect(dialog.queryByLabelText("Resolving merge conflicts")).toBeNull();
    c.dispose();
  });

  it("clears a prompt the user empties", async () => {
    const t = host({ "prompts.branch_name": "Start with the ticket number." });
    const c = await mount(t);
    const dialog = openSettings("Prompts");
    await tick();
    const field = dialog.getByLabelText("Branch names");
    fireEvent.change(field, { target: { value: "   " } });
    fireEvent.blur(field);
    await tick();
    expect(updates(t)).toEqual([{ "prompts.branch_name": null }]);
    c.dispose();
  });

  it("points to Instructions for everything else", async () => {
    const t = host();
    t.handlers["host.instructions"] = () => ({ host: { general: null, resolve_conflicts: null }, workspaces: {} });
    const c = await mount(t);
    const dialog = openSettings("Prompts");
    await tick();
    const pane = dialog.getByLabelText("Pull requests").closest("section")!.parentElement!;
    fireEvent.click(within(pane).getByRole("button", { name: "Instructions" }));
    await tick();
    expect(screen.getByRole("navigation", { name: "Settings sections" }).querySelector("[aria-current]")?.textContent).toContain(
      "Instructions",
    );
    c.dispose();
  });

  it("keeps a prompt too long for the host from being sent", async () => {
    const t = host();
    const c = await mount(t);
    const dialog = openSettings("Prompts");
    await tick();
    const field = dialog.getByLabelText("Pull requests");
    fireEvent.change(field, { target: { value: "é".repeat(MAX_PROMPT_BYTES / 2 + 1) } });
    fireEvent.blur(field);
    await tick();
    expect(dialog.getByRole("alert").textContent).toContain("Too long");
    expect(updates(t)).toEqual([]);
    c.dispose();
  });

  it("is not offered by a host without prompt settings", async () => {
    const t = host();
    t.handlers["host.hello"] = () =>
      hostHello({ capabilities: CLIENT_CAPABILITIES.filter((x) => x !== "settings.prompts") });
    const c = await mount(t);
    fireEvent.click(openMenu().getByRole("menuitem", { name: /^Settings/ }));
    const nav = within(screen.getByRole("navigation", { name: "Settings sections" }));
    expect(nav.queryByText("Prompts")).toBeNull();
    expect(nav.getByText("Git")).toBeTruthy();
    c.dispose();
  });
});
