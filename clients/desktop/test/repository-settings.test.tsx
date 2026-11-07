import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  CLIENT_CAPABILITIES,
  HostClient,
  HostController,
  MAX_SCRIPT_BYTES,
  type RepositorySettings,
  type WorkspaceInfo,
} from "@zwiim/sdk";
import { Workbench } from "@/App";
import { checkPreviews, formatEnv, parseEnv, parsePatterns } from "@/settings/repository";
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

function root(id: string, name: string, over: Partial<WorkspaceInfo> = {}): WorkspaceInfo {
  return {
    workspace_id: id,
    name,
    alias: null,
    kind: "root",
    parent_workspace_id: null,
    is_git_repo: true,
    branch: "main",
    base_branch: null,
    base_commit: null,
    removed: false,
    display_path: `~/Projects/${name}`,
    ...over,
  };
}

type Values = RepositorySettings["values"];

/** A Host with `workspaces`, keeping each repository's settings; an update merges into them. */
function host(workspaces: WorkspaceInfo[], scripts: Record<string, Values> = {}) {
  const t = new FakeTransport();
  t.handlers["workspace.list"] = () => workspaces;
  t.handlers["repository.settings.get"] = (p: { workspace_id: string }) => ({ values: { ...scripts[p.workspace_id] } });
  t.handlers["repository.settings.update"] = (p: { workspace_id: string; values: Values }) => {
    scripts[p.workspace_id] = { ...scripts[p.workspace_id], ...p.values };
    return { values: { ...scripts[p.workspace_id] } };
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

const updates = (t: FakeTransport) => t.calls.filter((c) => c.op === "repository.settings.update").map((c) => c.params);

describe("repository settings", () => {
  it("imports Conductor scripts and shows them, and says when nothing was set", async () => {
    const t = host([root("w1", "zwiim")]);
    let imported = ["scripts.setup"];
    t.handlers["repository.settings.import"] = () => {
      const result = { values: { "scripts.setup": "npm install" }, imported };
      imported = [];
      return result;
    };
    const c = await mount(t);
    const dialog = openSettings("Repository");
    await tick();

    fireEvent.click(dialog.getByRole("button", { name: "Import from Conductor" }));
    await tick();
    expect((dialog.getByLabelText("Setup script") as HTMLTextAreaElement).value).toBe("npm install");
    expect(dialog.getByText(/Imported 1 script/)).toBeTruthy();

    fireEvent.click(dialog.getByRole("button", { name: "Import from Conductor" }));
    await tick();
    expect(dialog.getByText(/Nothing imported/)).toBeTruthy();
    c.dispose();
  });

  it("does not offer the import to a host without it", async () => {
    const t = host([root("w1", "zwiim")]);
    t.handlers["host.hello"] = () =>
      hostHello({
        capabilities: CLIENT_CAPABILITIES.filter((x) => x !== "repository.settings_import"),
      });
    const c = await mount(t);
    const dialog = openSettings("Repository");
    await tick();
    expect(dialog.queryByRole("button", { name: "Import from Conductor" })).toBeNull();
    c.dispose();
  });

  it("shows a repository's scripts, saves them on leaving a field and switches repositories", async () => {
    const t = host(
      [root("w1", "zwiim"), root("w2", "site"), root("w3", "notes", { is_git_repo: false })],
      { w1: { "scripts.setup": "npm install" }, w2: { "scripts.archive": "docker compose down" } },
    );
    const c = await mount(t);
    const dialog = openSettings("Repository");
    await tick();
    const picker = dialog.getByLabelText("Repository") as HTMLSelectElement;
    // Only repositories can have worktrees, so only they are offered.
    expect([...picker.options].map((o) => o.textContent)).toEqual(["zwiim", "site"]);
    expect((dialog.getByLabelText("Setup script") as HTMLTextAreaElement).value).toBe("npm install");

    const archive = dialog.getByLabelText("Archive script");
    fireEvent.change(archive, { target: { value: "  ./scripts/stop.sh\n" } });
    fireEvent.blur(archive);
    await tick();
    fireEvent.blur(archive);
    await tick();
    expect(updates(t)).toEqual([{ workspace_id: "w1", values: { "scripts.archive": "./scripts/stop.sh" } }]);

    fireEvent.change(picker, { target: { value: "w2" } });
    await tick();
    expect((dialog.getByLabelText("Setup script") as HTMLTextAreaElement).value).toBe("");
    const other = dialog.getByLabelText("Archive script") as HTMLTextAreaElement;
    expect(other.value).toBe("docker compose down");
    fireEvent.change(other, { target: { value: " " } });
    fireEvent.blur(other);
    await tick();
    expect(updates(t).at(-1)).toEqual({ workspace_id: "w2", values: { "scripts.archive": null } });
    c.dispose();
  });

  it("opens on the repository of the worktree the window shows", async () => {
    const worktree = root("wt", "pune", { kind: "worktree", parent_workspace_id: "w2", display_path: null });
    const t = host([worktree, root("w1", "zwiim"), root("w2", "site")], { w2: { "scripts.setup": "make" } });
    const c = await mount(t);
    const dialog = openSettings("Repository");
    await tick();
    expect((dialog.getByLabelText("Repository") as HTMLSelectElement).value).toBe("w2");
    expect((dialog.getByLabelText("Setup script") as HTMLTextAreaElement).value).toBe("make");
    c.dispose();
  });

  it("opens on the repository whose menu asked, from tab to tab", async () => {
    const worktree = root("wt", "pune", { kind: "worktree", parent_workspace_id: "w1", display_path: null });
    const t = host([worktree, root("w1", "zwiim"), root("w2", "site")], { w2: { "scripts.setup": "make" } });
    const c = await mount(t);
    fireEvent.click(screen.getByRole("button", { name: "site actions" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Configuration" }));
    await tick();
    const dialog = within(screen.getByRole("dialog", { name: "Settings" }));
    expect((dialog.getByLabelText("Repository") as HTMLSelectElement).value).toBe("w2");
    expect((dialog.getByLabelText("Setup script") as HTMLTextAreaElement).value).toBe("make");
    const sections = within(dialog.getByRole("navigation", { name: "Settings sections" }));
    fireEvent.click(sections.getByText("General"));
    fireEvent.click(sections.getByText("Repository"));
    await tick();
    expect((dialog.getByLabelText("Repository") as HTMLSelectElement).value).toBe("w2");
    c.dispose();
  });

  it("keeps the run script, with the worktree's port", async () => {
    const t = host([root("w1", "zwiim")], { w1: { "scripts.run": "bun dev" } });
    const c = await mount(t);
    const dialog = openSettings("Repository");
    await tick();
    const field = dialog.getByLabelText("Run script") as HTMLTextAreaElement;
    expect(field.value).toBe("bun dev");
    expect(field.placeholder).toContain("$ZWIIM_PORT");
    fireEvent.change(field, { target: { value: "bun dev --port $ZWIIM_PORT" } });
    fireEvent.blur(field);
    await tick();
    expect(updates(t)).toEqual([{ workspace_id: "w1", values: { "scripts.run": "bun dev --port $ZWIIM_PORT" } }]);
    c.dispose();
  });

  it("keeps a script too long for the host from being sent", async () => {
    const t = host([root("w1", "zwiim")]);
    const c = await mount(t);
    const dialog = openSettings("Repository");
    await tick();
    const field = dialog.getByLabelText("Setup script");
    fireEvent.change(field, { target: { value: "é".repeat(MAX_SCRIPT_BYTES / 2 + 1) } });
    fireEvent.blur(field);
    await tick();
    expect(dialog.getByRole("alert").textContent).toContain("Too long");
    expect(updates(t)).toEqual([]);
    c.dispose();
  });

  it("picks the files a new worktree gets, .env files when unset", async () => {
    const t = host([root("w1", "zwiim")]);
    const c = await mount(t);
    const dialog = openSettings("Repository");
    await tick();
    const field = dialog.getByLabelText("Files to copy") as HTMLTextAreaElement;
    expect(field.value).toBe(".env*");
    const edit = async (value: string) => {
      fireEvent.change(field, { target: { value } });
      fireEvent.blur(field);
      await tick();
    };

    await edit(".env*\n");
    expect(updates(t)).toEqual([]);
    await edit("  .env*\n# keys\n\nconfig/*.local.json ");
    expect(updates(t).at(-1)).toEqual({ workspace_id: "w1", values: { "files.copy": [".env*", "config/*.local.json"] } });
    await edit("");
    expect(updates(t).at(-1)).toEqual({ workspace_id: "w1", values: { "files.copy": [] } });
    // Back to the default is unset, so it follows the default.
    await edit(".env*");
    expect(updates(t).at(-1)).toEqual({ workspace_id: "w1", values: { "files.copy": null } });
    await edit("x".repeat(300));
    expect(dialog.getByRole("alert").textContent).toContain("Pattern too long");
    expect(updates(t)).toHaveLength(3);
    c.dispose();
  });

  it("hides variable values until asked and saves them as .env lines", async () => {
    const t = host([root("w1", "zwiim")], { w1: { env: { API_KEY: "s3cret", API_URL: "http://localhost:4000" } } });
    const c = await mount(t);
    const dialog = openSettings("Repository");
    await tick();
    const list = dialog.getByRole("list", { name: "Environment variables" });
    expect(list.textContent).toContain("API_KEY=");
    expect(dialog.queryByText(/s3cret/)).toBeNull();

    fireEvent.click(dialog.getByRole("button", { name: "Show and edit" }));
    const field = dialog.getByLabelText("Environment variables") as HTMLTextAreaElement;
    expect(field.value).toBe("API_KEY=s3cret\nAPI_URL=http://localhost:4000");
    fireEvent.change(field, { target: { value: "API_KEY=s3cret\nZWIIM_ROOT_PATH=/x\nnope" } });
    fireEvent.blur(field);
    await tick();
    const alert = dialog.getByRole("alert").textContent;
    expect(alert).toContain("Line 2: names starting with ZWIIM_");
    expect(alert).toContain("Line 3: expected NAME=value");
    expect(updates(t)).toEqual([]);

    fireEvent.change(field, { target: { value: '# local\nexport API_KEY="new key"\nDEBUG=1' } });
    fireEvent.click(dialog.getByRole("button", { name: "Hide values" }));
    await tick();
    expect(updates(t)).toEqual([{ workspace_id: "w1", values: { env: { API_KEY: "new key", DEBUG: "1" } } }]);
    expect(dialog.getByRole("list", { name: "Environment variables" }).textContent).toContain("DEBUG=");

    fireEvent.click(dialog.getByRole("button", { name: "Show and edit" }));
    const again = dialog.getByLabelText("Environment variables");
    fireEvent.change(again, { target: { value: "" } });
    fireEvent.blur(again);
    await tick();
    expect(updates(t).at(-1)).toEqual({ workspace_id: "w1", values: { env: null } });
    c.dispose();
  });

  it("keeps preview URLs, named or not, in order, from a row each", async () => {
    const t = host([root("w1", "zwiim")], { w1: { "preview.urls": [{ name: "App", url: "http://localhost:$ZWIIM_PORT" }] } });
    const c = await mount(t);
    const dialog = openSettings("Repository");
    await tick();
    const name = (i: number) => dialog.getByLabelText(`Name of preview ${i}`) as HTMLInputElement;
    const url = (i: number) => dialog.getByLabelText(`URL of preview ${i}`) as HTMLInputElement;
    expect([name(1).value, url(1).value]).toEqual(["App", "http://localhost:$ZWIIM_PORT"]);
    const type = async (field: HTMLInputElement, value: string) => {
      fireEvent.change(field, { target: { value } });
      fireEvent.blur(field);
      await tick();
    };

    await type(url(1), "http://localhost:$ZWIIM_PORT");
    expect(updates(t)).toEqual([]);
    fireEvent.click(dialog.getByRole("button", { name: "Add URL" }));
    await type(url(2), "localhost:$((ZWIIM_PORT + 1))/admin");
    expect(updates(t).at(-1)).toEqual({
      workspace_id: "w1",
      values: {
        "preview.urls": [
          { name: "App", url: "http://localhost:$ZWIIM_PORT" },
          { url: "http://localhost:$((ZWIIM_PORT + 1))/admin" },
        ],
      },
    });

    fireEvent.click(dialog.getByRole("button", { name: "Add URL" }));
    await type(name(3), "Docs");
    expect(dialog.getByRole("alert").textContent).toBe("Docs: add its URL.");
    await type(url(3), "http://a b");
    expect(dialog.getByRole("alert").textContent).toBe("Docs: a URL has no spaces.");
    expect(url(3).getAttribute("aria-invalid")).toBe("true");
    expect(updates(t)).toHaveLength(1);

    fireEvent.click(dialog.getByRole("button", { name: "Remove preview 3" }));
    await tick();
    expect(dialog.queryByRole("alert")).toBeNull();
    fireEvent.click(dialog.getByRole("button", { name: "Remove preview 2" }));
    await tick();
    fireEvent.click(dialog.getByRole("button", { name: "Remove preview 1" }));
    await tick();
    expect(updates(t).at(-1)).toEqual({ workspace_id: "w1", values: { "preview.urls": null } });
    expect([name(1).value, url(1).value]).toEqual(["", ""]);
    c.dispose();
  });

  it("reads .env lines back as it writes them", () => {
    const env = { A: " padded ", B: '"quoted"', C: "a=b#c", D: "" };
    expect(parseEnv(formatEnv(env))).toEqual({ value: env, problems: [] });
    expect(parseEnv("export  X='1'\nX=2").value).toEqual({ X: "2" });
    expect(parseEnv("1X=1").problems).toEqual(["Line 1: 1X is not a variable name."]);
    expect(parsePatterns("a\n  # b\n\nc ").value).toEqual(["a", "c"]);
    expect(
      checkPreviews([
        { name: " Web: v2 ", url: "HTTP://x/?a=http://y" },
        { name: "", url: "" },
        { name: "", url: "z:8080" },
      ]),
    ).toEqual({
      value: [{ name: "Web: v2", url: "HTTP://x/?a=http://y" }, { url: "http://z:8080" }],
      problems: [],
      bad: [],
    });
    expect(checkPreviews(Array.from({ length: 11 }, (_, i) => ({ name: "", url: `h${i}` }))).problems).toEqual([
      "At most 10 URLs.",
    ]);
  });

  it("says what to do when there is no repository", async () => {
    const t = host([root("w3", "notes", { is_git_repo: false })]);
    const c = await mount(t);
    const dialog = openSettings("Repository");
    await tick();
    expect(dialog.getByText(/Add a Git repository/)).toBeTruthy();
    expect(t.calls.some((x) => x.op === "repository.settings.get")).toBe(false);
    c.dispose();
  });

  it("is not offered by a host without repository settings", async () => {
    const t = host([root("w1", "zwiim")]);
    t.handlers["host.hello"] = () =>
      hostHello({ capabilities: CLIENT_CAPABILITIES.filter((x) => x !== "repository.settings") });
    const c = await mount(t);
    fireEvent.click(openMenu().getByRole("menuitem", { name: /^Settings/ }));
    const nav = within(screen.getByRole("navigation", { name: "Settings sections" }));
    expect(nav.queryByText("Repository")).toBeNull();
    expect(nav.getByText("Prompts")).toBeTruthy();
    c.dispose();
  });
});
