import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  CLIENT_CAPABILITIES,
  HostClient,
  HostController,
  ZwiimClientError,
  type UserInstructions,
  type WorkspaceInfo,
} from "@zwiim/sdk";
import { Workbench } from "@/App";
import type { NativeBridge } from "@/lib/bridge";
import { HostProvider } from "@/lib/host-context";
import { rememberSelection, rememberWelcomed } from "@/lib/prefs";
import { FakeTransport, hostHello, tick } from "./fakes";
import { openMenu } from "./ui";

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
    signIn: vi.fn(async () => ({ user_id: "u-me", email: "me@example.com" })),
    cancelSignIn: vi.fn(async () => {}),
    signOut: vi.fn(async () => {}),
  };
}

const workspace = (extra: Partial<WorkspaceInfo>): WorkspaceInfo => ({
  workspace_id: "root",
  name: "repo",
  alias: null,
  kind: "root",
  parent_workspace_id: null,
  branch: "main",
  base_branch: null,
  base_commit: null,
  removed: false,
  is_git_repo: true,
  display_path: null,
  ...extra,
});

const root = workspace({});
const worktree = workspace({
  workspace_id: "a",
  name: "repo-a",
  kind: "worktree",
  parent_workspace_id: "root",
  branch: "zwiim/a",
  base_branch: "main",
  base_commit: "0123456",
});

/** A Host with one repository and its worktree "a", and these instructions saved. */
function instructionsHost(saved: UserInstructions) {
  const t = new FakeTransport();
  t.handlers["workspace.list"] = () => [root, worktree];
  t.handlers["host.instructions"] = () => saved;
  t.handlers["host.instructions.set"] = () => saved;
  return t;
}

async function mount(t: FakeTransport) {
  // Desktop reopens worktree "a", as it would after a launch there.
  rememberSelection("h1", { workspaceId: "a", sessionId: null });
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

function openInstructions() {
  fireEvent.click(openMenu().getByRole("menuitem", { name: /^Settings/ }));
  const dialog = within(screen.getByRole("dialog", { name: "Settings" }));
  fireEvent.click(within(dialog.getByRole("navigation", { name: "Settings sections" })).getByText("Instructions"));
  return dialog;
}

const scope = (name: string) => within(screen.getByRole("region", { name }));
const field = (region: string, label: string) =>
  scope(region).getByRole("textbox", { name: label }) as HTMLTextAreaElement;
const save = (region: string) => scope(region).getByRole("button", { name: "Save" }) as HTMLButtonElement;
const sets = (t: FakeTransport) => t.calls.filter((c) => c.op === "host.instructions.set").map((c) => c.params);

const empty: UserInstructions = { host: { general: null, resolve_conflicts: null }, workspaces: {} };

describe("instructions settings", () => {
  it("offers Instructions only when the Host serves them", async () => {
    const older = instructionsHost(empty);
    older.handlers["host.hello"] = () =>
      hostHello({ capabilities: CLIENT_CAPABILITIES.filter((c) => c !== "host.instructions") });
    const c = await mount(older);
    fireEvent.click(openMenu().getByRole("menuitem", { name: /^Settings/ }));
    const nav = within(screen.getByRole("navigation", { name: "Settings sections" }));
    expect(nav.queryByText("Instructions")).toBeNull();
    expect(older.calls.some((x) => x.op === "host.instructions")).toBe(false);
    c.dispose();
  });

  it("edits the Host-wide texts, sending both, with a byte counter and when they apply", async () => {
    const t = instructionsHost({ ...empty, host: { general: "Be brief.", resolve_conflicts: null } });
    const c = await mount(t);
    openInstructions();
    await tick();
    expect(field("Every workspace", "Instructions for every session").value).toBe("Be brief.");
    expect(scope("Every workspace").getByText("9 / 8192 bytes")).toBeTruthy();
    expect(scope("Every workspace").getByText("Applies to new sessions, and to open ones after a few idle minutes.")).toBeTruthy();
    expect(save("Every workspace").disabled).toBe(true);
    fireEvent.change(field("Every workspace", "When resolving conflicts"), { target: { value: "Keep our lockfile." } });
    expect(save("Every workspace").disabled).toBe(false);
    fireEvent.click(save("Every workspace"));
    await tick();
    expect(sets(t)).toEqual([{ scope: "host", general: "Be brief.", resolve_conflicts: "Keep our lockfile." }]);
    c.dispose();
  });

  it("refuses to save over 8 KiB, and shows the Host's too_large inline", async () => {
    const t = instructionsHost(empty);
    t.handlers["host.instructions.set"] = () => {
      throw new ZwiimClientError("too_large", "An instruction text is larger than 8 KiB", 413);
    };
    const c = await mount(t);
    openInstructions();
    await tick();
    fireEvent.change(field("Every workspace", "Instructions for every session"), { target: { value: "é".repeat(4097) } });
    expect(scope("Every workspace").getByText("8194 / 8192 bytes").className).toContain("text-destructive");
    expect(scope("Every workspace").getByText("Too long to save. Shorten it to 8 KiB.")).toBeTruthy();
    expect(save("Every workspace").disabled).toBe(true);
    fireEvent.change(field("Every workspace", "Instructions for every session"), { target: { value: "Fine." } });
    fireEvent.click(save("Every workspace"));
    await tick();
    expect(scope("Every workspace").getByRole("alert").textContent).toBe("Each text can be at most 8 KiB.");
    c.dispose();
  });

  it("edits the selected worktree's repository, which its worktrees share", async () => {
    const t = instructionsHost({ ...empty, workspaces: { root: { general: null, resolve_conflicts: "Prefer main's schema." } } });
    const c = await mount(t);
    openInstructions();
    await tick();
    const repo = scope("repo");
    expect(repo.getByText("Its worktrees use these too.")).toBeTruthy();
    expect(field("repo", "When resolving conflicts").value).toBe("Prefer main's schema.");
    fireEvent.change(field("repo", "Instructions for every session"), { target: { value: "Run cargo test." } });
    fireEvent.click(save("repo"));
    await tick();
    expect(sets(t)).toEqual([
      { scope: "workspace", workspace_id: "root", general: "Run cargo test.", resolve_conflicts: "Prefer main's schema." },
    ]);
    c.dispose();
  });
});
