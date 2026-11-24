import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CLIENT_CAPABILITIES, HostClient, HostController, type PendingApproval, type WorkspaceInfo } from "@zwiim/sdk";
import { Workbench } from "@/App";
import type { NativeBridge } from "@/lib/bridge";
import { HostProvider } from "@/lib/host-context";
import { rememberWelcomed } from "@/lib/prefs";
import { FakeTransport, hostHello, tick } from "./fakes";

const automations = vi.hoisted(() => ({ shown: true }));
vi.mock("@/panels/automations/shown", () => ({
  get AUTOMATIONS_SHOWN() {
    return automations.shown;
  },
}));

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
    accountStatus: vi.fn(async () => ({ available: false, account: null })),
    signIn: vi.fn(async () => ({ user_id: "u1", email: null })),
    cancelSignIn: vi.fn(async () => {}),
    signOut: vi.fn(async () => {}),
  };
}

const ws = (id: string): WorkspaceInfo => ({
  workspace_id: id,
  name: `repo-${id}`,
  alias: null,
  kind: "root",
  parent_workspace_id: null,
  branch: "main",
  base_branch: null,
  base_commit: null,
  removed: false,
  is_git_repo: true,
  display_path: null,
});

function approval(id: string): PendingApproval {
  return {
    approval_id: id,
    run_id: "r1",
    item_id: `i-${id}`,
    kind: "submit_application",
    summary: "Submit the application",
    fields: [],
    submit_target: { role: "button", name: "Submit" },
    created_at: "2026-11-21T10:00:00Z",
  };
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

function host(capabilities = CLIENT_CAPABILITIES.filter((c) => c !== "workspace.publish")) {
  const t = new FakeTransport();
  t.handlers["host.hello"] = () => hostHello({ capabilities });
  t.handlers["workspace.list"] = () => [ws("w1")];
  t.handlers["playbook.list"] = () => [];
  t.handlers["run.list"] = () => ({ runs: [], next_cursor: null });
  const snapshot = t.snapshot.bind(t);
  t.snapshot = () => ({ ...snapshot(), approvals: [approval("a1"), approval("a2")] });
  return t;
}

describe("Automations in the sidebar", () => {
  it("the row is hidden without the playbooks capability", async () => {
    const t = host(CLIENT_CAPABILITIES.filter((c) => c !== "workspace.publish" && c !== "playbooks"));
    const c = await mount(t);
    expect(screen.queryByTestId("automations-row")).toBeNull();
    c.dispose();
  });

  it("the row is hidden in a release without Automations", async () => {
    automations.shown = false;
    try {
      const c = await mount(host());
      expect(screen.queryByTestId("automations-row")).toBeNull();
      c.dispose();
    } finally {
      automations.shown = true;
    }
  });

  it("the row shows waiting approvals and opens the canvas", async () => {
    const t = host();
    const c = await mount(t);
    const row = screen.getByTestId("automations-row");
    expect(row.textContent).toContain("Automations");
    expect(row.textContent).toContain("2");

    fireEvent.click(row);
    await tick();
    expect(screen.getByTestId("automations-header")).toBeTruthy();
    expect(row.getAttribute("aria-current")).toBe("true");

    fireEvent.click(document.querySelector('[data-row="w1"]')!);
    await tick();
    expect(screen.queryByTestId("automations-header")).toBeNull();
    expect(row.getAttribute("aria-current")).toBeNull();
    c.dispose();
  });
});
