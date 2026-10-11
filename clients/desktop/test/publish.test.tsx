import { useState } from "react";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  CLIENT_CAPABILITIES,
  HostClient,
  HostController,
  ZwiimClientError,
  type SessionSummary,
  type WorkspaceInfo,
  type WorkspaceInspection,
} from "@zwiim/sdk";
import { HostProvider } from "@/lib/host-context";
import { PublishDialog, type PublishDialogProps } from "@/panels/PublishDialog";
import { Sidebar, type SidebarProps } from "@/panels/Sidebar";
import { FakeTransport, hostHello, tick } from "./fakes";
import { openProject } from "./ui";

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

const added: WorkspaceInfo = {
  workspace_id: "w1",
  name: "my-app",
  alias: null,
  kind: "root",
  parent_workspace_id: null,
  branch: "main",
  base_branch: null,
  base_commit: null,
  removed: false,
  is_git_repo: true,
  display_path: null,
};

const plain = (extra: Partial<WorkspaceInspection> = {}): WorkspaceInspection => ({
  git: "none",
  branch: "main",
  suggested_name: "my-app",
  first_commit: { files: 412, bytes: 1_832_000, at_least: false, large: false },
  github: {
    available: true,
    login: "Ashish1101",
    owners: [
      { login: "Ashish1101", kind: "user" },
      { login: "mazik-labs", kind: "org" },
    ],
  },
  ...extra,
});

/** A Host whose `github.repo_available` answers from `taken` (owner/name pairs). */
function host(taken: string[] = []) {
  const t = new FakeTransport();
  t.handlers["github.repo_available"] = (p: { owner: string; name: string }) => ({
    available: !taken.includes(`${p.owner}/${p.name}`),
  });
  t.handlers["workspace.add"] = () => added;
  return t;
}

async function mount(t: FakeTransport, props: Partial<PublishDialogProps> = {}) {
  const client = new HostClient({ transport: t });
  const controller = new HostController(client);
  client.start();
  await tick();
  const all: PublishDialogProps = {
    subject: { kind: "path", path: "/Users/me/my-app" },
    inspection: plain(),
    remote: false,
    onClose: vi.fn(),
    onAdded: vi.fn(),
    ...props,
  };
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
  render(
    <HostProvider value={controller}>
      <PublishDialog {...all} />
    </HostProvider>,
  );
  await tick();
  return all;
}

/** Let the 300 ms name check fire and its answer land. */
async function settle() {
  await act(async () => void vi.advanceTimersByTime(300));
  await tick();
}

const checks = (t: FakeTransport) => t.calls.filter((c) => c.op === "github.repo_available").map((c) => c.params);
const adds = (t: FakeTransport) => t.calls.filter((c) => c.op === "workspace.add").map((c) => c.params);
const nameField = () => screen.getByLabelText("Repository name") as HTMLInputElement;
const confirm = () => screen.getByRole("button", { name: /create repo/i }) as HTMLButtonElement;
const text = (s: string) => (_: string, el: Element | null) => el?.textContent === s;
const button = (name: string) => screen.getByRole("button", { name }) as HTMLButtonElement;
const buttons = () => screen.getAllByRole("button").map((b) => b.textContent);
const inspects = (t: FakeTransport) => t.calls.filter((c) => c.op === "workspace.inspect").map((c) => c.params);
const offline = (reason: "gh_unavailable" | "gh_unauthenticated" | "github_unreachable") =>
  plain({ github: { available: false, reason } });
const large = { files: 12_431, bytes: 1.8 * 1024 ** 3, at_least: false, large: true };

describe("PublishDialog", () => {
  it("asks to create a private repository, with the plain folder's steps", async () => {
    await mount(host());
    expect(screen.getByRole("dialog", { name: "Create a private GitHub repo?" })).toBeTruthy();
    expect(screen.getByText("Create a private GitHub repo?")).toBeTruthy();
    expect(
      screen.getByText(text("Zwiim will run git init, create a private GitHub repo, add it as origin, and push main.")),
    ).toBeTruthy();
    expect(screen.getByRole("button", { name: "Initialize Git and create repo" })).toBeTruthy();
  });

  it("words the steps for a repository without an origin, and an unborn one", async () => {
    await mount(host(), {
      inspection: plain({
        git: "no_origin",
        branch: "dev",
        first_commit: null,
      }),
    });
    expect(
      screen.getByText(text("Zwiim will create a private GitHub repo, add it as origin, and push dev.")),
    ).toBeTruthy();
    expect(screen.getByRole("button", { name: "Create repo" })).toBeTruthy();
    cleanup();
    vi.useRealTimers();

    await mount(host(), {
      inspection: plain({ git: "no_origin", branch: "trunk" }),
    });
    expect(
      screen.getByText(
        text("Zwiim will make a first commit, create a private GitHub repo, add it as origin, and push trunk."),
      ),
    ).toBeTruthy();
  });

  it("lists the login first, then organizations, and prefills the name focused and selected", async () => {
    await mount(host());
    const owner = screen.getByLabelText("Owner") as HTMLSelectElement;
    expect([...owner.options].map((o) => o.value)).toEqual(["Ashish1101", "mazik-labs"]);
    expect(owner.value).toBe("Ashish1101");
    const name = nameField();
    expect(name.value).toBe("my-app");
    expect(document.activeElement).toBe(name);
    expect([name.selectionStart, name.selectionEnd]).toEqual([0, "my-app".length]);
  });

  it("names the repository it will create, live with every keystroke and owner change", async () => {
    await mount(host());
    expect(screen.getByText("Will create Ashish1101/my-app")).toBeTruthy();
    fireEvent.change(nameField(), { target: { value: "my-ap" } });
    expect(screen.getByText("Will create Ashish1101/my-ap")).toBeTruthy();
    fireEvent.change(screen.getByLabelText("Owner"), {
      target: { value: "mazik-labs" },
    });
    expect(screen.getByText("Will create mazik-labs/my-ap")).toBeTruthy();
  });

  it("checks the name once, 300 ms after the last change, and enables Create only when free", async () => {
    const t = host();
    await mount(t);
    await settle();
    expect(checks(t)).toEqual([{ owner: "Ashish1101", name: "my-app" }]);
    // DESIGN.md: success reads in `done`; checking stays muted.
    expect(screen.getByText("Repository name is available.").className).toContain("text-done");
    expect(confirm().disabled).toBe(false);
    // DESIGN.md's `primary` button is the accent (`brand`); Cancel is `secondary`.
    expect(confirm().className).toContain("bg-brand");
    expect(button("Cancel").className).toContain("bg-secondary");

    fireEvent.change(nameField(), { target: { value: "n" } });
    fireEvent.change(nameField(), { target: { value: "ne" } });
    fireEvent.change(nameField(), { target: { value: "new" } });
    expect(screen.getByText("Checking…").className).toContain("text-muted-foreground");
    expect(confirm().disabled).toBe(true);
    await act(async () => void vi.advanceTimersByTime(299));
    expect(checks(t)).toHaveLength(1);
    await settle();
    expect(checks(t)).toEqual([
      { owner: "Ashish1101", name: "my-app" },
      { owner: "Ashish1101", name: "new" },
    ]);
    expect(confirm().disabled).toBe(false);
  });

  it("marks a taken name and keeps Create disabled", async () => {
    await mount(host(["Ashish1101/my-app"]));
    await settle();
    expect(screen.getByText(text("Ashish1101/my-app already exists."))).toBeTruthy();
    expect(confirm().disabled).toBe(true);
  });

  it("says it couldn't check when the check fails, and keeps Create disabled", async () => {
    const t = host();
    t.handlers["github.repo_available"] = () => {
      throw new Error("offline");
    };
    await mount(t);
    await settle();
    expect(screen.getByText("Couldn't check this name.")).toBeTruthy();
    expect(confirm().disabled).toBe(true);
  });

  it("shows the rule a bad name breaks without asking the Host", async () => {
    const t = host();
    await mount(t);
    fireEvent.change(nameField(), { target: { value: "a b" } });
    await settle();
    expect(checks(t)).toEqual([]);
    expect(screen.getByText("Repository names use letters, digits, '.', '-' and '_' (at most 100).")).toBeTruthy();
    expect(confirm().disabled).toBe(true);
    for (const bad of ["..", ".", "", "x".repeat(101)]) {
      fireEvent.change(nameField(), { target: { value: bad } });
      await settle();
      expect(checks(t)).toEqual([]);
      expect(confirm().disabled).toBe(true);
    }
  });

  it("drops a slower answer for an older name", async () => {
    const t = host(["Ashish1101/old"]);
    const pending: Record<string, (v: { available: boolean }) => void> = {};
    t.handlers["github.repo_available"] = (p: { name: string }) =>
      new Promise((resolve) => {
        pending[p.name] = resolve;
      });
    await mount(t, { inspection: plain({ suggested_name: "old" }) });
    await settle();
    fireEvent.change(nameField(), { target: { value: "fresh" } });
    await settle();
    expect(checks(t).map((c) => c.name)).toEqual(["old", "fresh"]);

    await act(async () => pending.fresh!({ available: true }));
    await tick();
    expect(screen.getByText("Repository name is available.")).toBeTruthy();
    // The check for "old" answers last, saying it is taken: it no longer counts.
    await act(async () => pending.old!({ available: false }));
    await tick();
    expect(screen.queryByText(/already exists/)).toBeNull();
    expect(screen.getByText("Repository name is available.")).toBeTruthy();
    expect(confirm().disabled).toBe(false);
  });

  it("drops an answer for the same name asked again before it came back", async () => {
    const t = host();
    const answers: ((v: { available: boolean }) => void)[] = [];
    t.handlers["github.repo_available"] = () => new Promise((resolve) => answers.push(resolve));
    await mount(t);
    await settle();
    fireEvent.change(nameField(), { target: { value: "other" } });
    fireEvent.change(nameField(), { target: { value: "my-app" } });
    await settle();
    expect(answers).toHaveLength(2);
    await act(async () => answers[1]!({ available: false }));
    await tick();
    await act(async () => answers[0]!({ available: true }));
    await tick();
    expect(screen.getByText(text("Ashish1101/my-app already exists."))).toBeTruthy();
    expect(confirm().disabled).toBe(true);
  });

  it("creates exactly the owner and name in the fields, then hands back the workspace", async () => {
    const t = host();
    let finish: (w: WorkspaceInfo) => void = () => {};
    t.handlers["workspace.add"] = () => new Promise<WorkspaceInfo>((resolve) => (finish = resolve));
    const props = await mount(t);
    fireEvent.change(screen.getByLabelText("Owner"), {
      target: { value: "mazik-labs" },
    });
    fireEvent.change(nameField(), { target: { value: "tool" } });
    await settle();
    fireEvent.click(confirm());
    await tick();
    expect(adds(t)).toEqual([
      {
        path: "/Users/me/my-app",
        publish: { owner: "mazik-labs", name: "tool" },
      },
    ]);
    expect(screen.getByRole("button", { name: /Creating repo…/ })).toBeTruthy();
    expect(nameField().disabled).toBe(true);
    expect((screen.getByLabelText("Owner") as HTMLSelectElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Cancel" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.keyDown(document, { key: "Escape" });
    expect(props.onClose).not.toHaveBeenCalled();

    await act(async () => finish(added));
    await tick();
    expect(props.onAdded).toHaveBeenCalledWith(added);
    expect(screen.getByRole("button", { name: /Preparing workspace…/ })).toBeTruthy();
  });

  it("adds a listed root by its id", async () => {
    const t = host();
    await mount(t, { subject: { kind: "root", workspace_id: "r1" } });
    await settle();
    fireEvent.click(confirm());
    await tick();
    expect(adds(t)).toEqual([{ workspace_id: "r1", publish: { owner: "Ashish1101", name: "my-app" } }]);
  });

  it("presses Create on Enter only once the name is checked and free", async () => {
    const t = host();
    await mount(t);
    fireEvent.keyDown(nameField(), { key: "Enter" });
    await tick();
    expect(adds(t)).toEqual([]);
    await settle();
    fireEvent.keyDown(nameField(), { key: "Enter" });
    await tick();
    expect(adds(t)).toHaveLength(1);
  });

  it("states a large first commit and confirms it with the same click", async () => {
    const t = host();
    await mount(t, {
      inspection: plain({
        first_commit: {
          files: 12_431,
          bytes: 1.8 * 1024 ** 3,
          at_least: false,
          large: true,
        },
      }),
    });
    expect(screen.getByText(text("The first commit will hold 12,431 files (1.8 GB)."))).toBeTruthy();
    await settle();
    fireEvent.click(confirm());
    await tick();
    expect(adds(t)).toEqual([
      {
        path: "/Users/me/my-app",
        allow_large: true,
        publish: { owner: "Ashish1101", name: "my-app" },
      },
    ]);
  });

  it("shows no large line for a small first commit", async () => {
    await mount(host());
    expect(screen.queryByText(/The first commit will hold/)).toBeNull();
  });

  it("keeps the form and shows an add error under the buttons", async () => {
    const t = host();
    t.handlers["workspace.add"] = () => {
      throw new Error("Folder is gone");
    };
    const props = await mount(t);
    await settle();
    fireEvent.click(confirm());
    await tick();
    expect(screen.getByRole("alert").textContent).toBe("Folder is gone");
    expect(nameField().disabled).toBe(false);
    expect(props.onAdded).not.toHaveBeenCalled();
  });

  it("shows the large line when the add finds the folder grew too large, and confirms it with the next click", async () => {
    const t = host();
    t.handlers["workspace.add"] = (p: { allow_large?: boolean }) => {
      if (!p.allow_large) throw new ZwiimClientError("large_folder", "big", 409, { files: 12_431, bytes: 1.8 * 1024 ** 3, at_least: false });
      return added;
    };
    const props = await mount(t);
    await settle();
    fireEvent.click(confirm());
    await tick();
    expect(screen.getByText(text("The first commit will hold 12,431 files (1.8 GB)."), { selector: "p" })).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(props.onAdded).not.toHaveBeenCalled();
    expect(adds(t)).toEqual([{ path: "/Users/me/my-app", publish: { owner: "Ashish1101", name: "my-app" } }]);
    fireEvent.click(confirm());
    await tick();
    expect(adds(t)).toEqual([
      { path: "/Users/me/my-app", publish: { owner: "Ashish1101", name: "my-app" } },
      { path: "/Users/me/my-app", allow_large: true, publish: { owner: "Ashish1101", name: "my-app" } },
    ]);
    expect(props.onAdded).toHaveBeenCalledWith(added);
  });

  it("Cancel and Escape close the dialog and send nothing", async () => {
    const t = host();
    const props = await mount(t);
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    fireEvent.keyDown(document, { key: "Escape" });
    expect(props.onClose).toHaveBeenCalledTimes(2);
    await settle();
    expect(adds(t)).toEqual([]);
  });

  it("sends one add for a double confirm while the add is pending", async () => {
    const t = host();
    t.handlers["workspace.add"] = () => new Promise<WorkspaceInfo>(() => {});
    await mount(t);
    await settle();
    // In one batch, before React re-renders the disabled button.
    await act(async () => {
      confirm().click();
      confirm().click();
      fireEvent.keyDown(nameField(), { key: "Enter" });
    });
    await tick();
    expect(adds(t)).toHaveLength(1);
  });
});

describe("PublishDialog failure states", () => {
  it("names why GitHub is unavailable on this Mac, in place of the fields", async () => {
    const lines = {
      gh_unavailable: "GitHub CLI isn't installed on this Mac.",
      gh_unauthenticated: "GitHub CLI isn't signed in on this Mac. Run gh auth login, then try again.",
      github_unreachable: "Zwiim can't reach GitHub.",
    } as const;
    for (const [reason, line] of Object.entries(lines)) {
      const t = host();
      await mount(t, { inspection: offline(reason as keyof typeof lines) });
      expect(screen.getByText(text(line), { selector: "p" })).toBeTruthy();
      expect(screen.queryByLabelText("Repository name")).toBeNull();
      expect(screen.queryByLabelText("Owner")).toBeNull();
      expect(buttons()).toEqual(["Cancel", "Add without GitHub", "Try again"]);
      await settle();
      expect(checks(t)).toEqual([]);
      cleanup();
      vi.useRealTimers();
    }
  });

  it("says the host when the Host is another computer", async () => {
    await mount(host(), { inspection: offline("gh_unavailable"), remote: true });
    expect(screen.getByText("GitHub CLI isn't installed on the host.")).toBeTruthy();
    cleanup();
    vi.useRealTimers();
    await mount(host(), { inspection: offline("gh_unauthenticated"), remote: true });
    expect(
      screen.getByText(text("GitHub CLI isn't signed in on the host. Run gh auth login, then try again."), {
        selector: "p",
      }),
    ).toBeTruthy();
  });

  it("adds without GitHub: no publish, and allow_large only when the large line showed", async () => {
    const t = host();
    let finish: (w: WorkspaceInfo) => void = () => {};
    t.handlers["workspace.add"] = () => new Promise<WorkspaceInfo>((resolve) => (finish = resolve));
    const props = await mount(t, { inspection: offline("gh_unavailable") });
    fireEvent.click(button("Add without GitHub"));
    await tick();
    expect(adds(t)).toEqual([{ path: "/Users/me/my-app" }]);
    expect(button("Cancel").disabled).toBe(true);
    fireEvent.keyDown(document, { key: "Escape" });
    expect(props.onClose).not.toHaveBeenCalled();
    await act(async () => finish(added));
    await tick();
    expect(props.onAdded).toHaveBeenCalledWith(added);
    expect(screen.getByRole("button", { name: /Preparing workspace…/ })).toBeTruthy();
    cleanup();
    vi.useRealTimers();

    const t2 = host();
    await mount(t2, {
      subject: { kind: "root", workspace_id: "r1" },
      inspection: plain({ first_commit: large, github: { available: false, reason: "github_unreachable" } }),
    });
    expect(screen.getByText(text("The first commit will hold 12,431 files (1.8 GB)."), { selector: "p" })).toBeTruthy();
    fireEvent.click(button("Add without GitHub"));
    await tick();
    expect(adds(t2)).toEqual([{ workspace_id: "r1", allow_large: true }]);
  });

  it("Try again re-inspects the same folder and never adds", async () => {
    const t = host();
    t.handlers["workspace.inspect"] = () => plain();
    await mount(t, { inspection: offline("gh_unauthenticated") });
    fireEvent.click(button("Try again"));
    await tick();
    expect(inspects(t)).toEqual([{ path: "/Users/me/my-app" }]);
    expect(adds(t)).toEqual([]);
    expect(nameField().value).toBe("my-app");
    expect((screen.getByLabelText("Owner") as HTMLSelectElement).value).toBe("Ashish1101");
    await settle();
    expect(checks(t)).toEqual([{ owner: "Ashish1101", name: "my-app" }]);
    expect(confirm().disabled).toBe(false);
    cleanup();
    vi.useRealTimers();

    const t2 = host();
    t2.handlers["workspace.inspect"] = () => offline("gh_unauthenticated");
    await mount(t2, { subject: { kind: "root", workspace_id: "r1" }, inspection: offline("gh_unavailable") });
    fireEvent.click(button("Try again"));
    await tick();
    expect(inspects(t2)).toEqual([{ workspace_id: "r1" }]);
    expect(
      screen.getByText(text("GitHub CLI isn't signed in on this Mac. Run gh auth login, then try again."), {
        selector: "p",
      }),
    ).toBeTruthy();
    expect(adds(t2)).toEqual([]);
  });

  it("asks for a branch when HEAD is detached, with the unavailable form's buttons", async () => {
    const t = host();
    await mount(t, { inspection: plain({ git: "no_origin", branch: null, first_commit: null }) });
    expect(
      screen.getByText("This repository isn't on a branch. Check out a branch on this Mac, then try again."),
    ).toBeTruthy();
    expect(screen.queryByLabelText("Repository name")).toBeNull();
    expect(buttons()).toEqual(["Cancel", "Add without GitHub", "Try again"]);
    await settle();
    expect(checks(t)).toEqual([]);
    cleanup();
    vi.useRealTimers();

    await mount(host(), {
      inspection: plain({ git: "no_origin", branch: null, first_commit: null }),
      remote: true,
    });
    expect(
      screen.getByText("This repository isn't on a branch. Check out a branch on the host, then try again."),
    ).toBeTruthy();
  });

  it("turns into the unavailable form when the add fails with a GitHub CLI error", async () => {
    const t = host();
    t.handlers["workspace.add"] = () => {
      throw new ZwiimClientError("gh_unauthenticated", "GitHub CLI on the Host is not logged in", 422);
    };
    await mount(t);
    await settle();
    fireEvent.click(confirm());
    await tick();
    expect(
      screen.getByText(text("GitHub CLI isn't signed in on this Mac. Run gh auth login, then try again."), {
        selector: "p",
      }),
    ).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(buttons()).toEqual(["Cancel", "Add without GitHub", "Try again"]);
  });

  it("keeps the form when GitHub didn't create the repository, and offers Add without GitHub", async () => {
    const t = host();
    t.handlers["workspace.add"] = (p: { publish?: unknown }) => {
      if (p.publish) throw new ZwiimClientError("publish_failed", "GitHub did not", 422, { created_on_github: false });
      return added;
    };
    const props = await mount(t);
    await settle();
    fireEvent.click(confirm());
    await tick();
    expect(
      screen.getByText(text("GitHub didn't create Ashish1101/my-app. Try another owner or name."), { selector: "p" }),
    ).toBeTruthy();
    expect(nameField().getAttribute("aria-invalid")).toBe("true");
    expect(buttons()).toEqual(["Cancel", "Add without GitHub", "Initialize Git and create repo"]);
    // The click that tries again is still Create.
    expect(confirm().disabled).toBe(false);
    expect(screen.queryByRole("alert")).toBeNull();

    fireEvent.change(screen.getByLabelText("Owner"), { target: { value: "mazik-labs" } });
    await settle();
    expect(screen.queryByText(/GitHub didn't create/)).toBeNull();
    expect(screen.getByText("Repository name is available.")).toBeTruthy();

    fireEvent.click(button("Add without GitHub"));
    await tick();
    expect(adds(t)).toEqual([
      { path: "/Users/me/my-app", publish: { owner: "Ashish1101", name: "my-app" } },
      { path: "/Users/me/my-app" },
    ]);
    expect(props.onAdded).toHaveBeenCalledWith(added);
  });

  it("says the push failed after GitHub created the repository, and leads with Add without GitHub", async () => {
    const t = host();
    t.handlers["workspace.add"] = (p: { publish?: unknown }) => {
      if (p.publish) throw new ZwiimClientError("publish_failed", "pushing failed", 422, { created_on_github: true });
      return added;
    };
    const open = vi.spyOn(window, "open").mockImplementation(() => null);
    await mount(t);
    await settle();
    fireEvent.click(confirm());
    await tick();
    expect(
      screen.getByText(text("Ashish1101/my-app was created on GitHub, but pushing to it failed."), { selector: "p" }),
    ).toBeTruthy();
    expect(screen.queryByLabelText("Repository name")).toBeNull();
    expect(buttons()).toEqual(["Open on GitHub", "Cancel", "Try another name", "Add without GitHub"]);
    fireEvent.click(button("Open on GitHub"));
    await tick();
    expect(open).toHaveBeenCalledWith("https://github.com/Ashish1101/my-app", "_blank", "noopener,noreferrer");
    open.mockRestore();
    expect(adds(t)).toHaveLength(1);

    fireEvent.click(button("Try another name"));
    await tick();
    const name = nameField();
    expect(document.activeElement).toBe(name);
    expect([name.selectionStart, name.selectionEnd]).toEqual([0, "my-app".length]);
    expect(screen.getByText(text("Ashish1101/my-app already exists."), { selector: "p" })).toBeTruthy();
    expect(confirm().disabled).toBe(true);
  });

  it("keeps the name GitHub created taken, whatever the check answers", async () => {
    // GitHub's check lags behind the create: it answers available for every name.
    const t = host();
    t.handlers["workspace.add"] = (p: { publish?: unknown }) => {
      if (p.publish) throw new ZwiimClientError("publish_failed", "pushing failed", 422, { created_on_github: true });
      return added;
    };
    await mount(t);
    await settle();
    fireEvent.click(confirm());
    await tick();
    fireEvent.click(button("Try another name"));
    await settle();
    expect(screen.getByText(text("Ashish1101/my-app already exists."), { selector: "p" })).toBeTruthy();
    expect(confirm().disabled).toBe(true);
    expect(checks(t)).toEqual([{ owner: "Ashish1101", name: "my-app" }]);

    fireEvent.change(nameField(), { target: { value: "my-app-2" } });
    await settle();
    expect(screen.getByText("Repository name is available.")).toBeTruthy();
    expect(confirm().disabled).toBe(false);

    fireEvent.change(nameField(), { target: { value: "my-app" } });
    await settle();
    expect(screen.getByText(text("Ashish1101/my-app already exists."), { selector: "p" })).toBeTruthy();
    expect(confirm().disabled).toBe(true);
    expect(checks(t)).toEqual([
      { owner: "Ashish1101", name: "my-app" },
      { owner: "Ashish1101", name: "my-app-2" },
    ]);
    // The same name under another owner is a different repository.
    fireEvent.change(screen.getByLabelText("Owner"), { target: { value: "mazik-labs" } });
    await settle();
    expect(screen.getByText("Repository name is available.")).toBeTruthy();
    expect(confirm().disabled).toBe(false);
  });

  it("offers only Add repository when Try again finds the folder now has a remote", async () => {
    const t = host();
    t.handlers["workspace.inspect"] = () => plain({ git: "origin", first_commit: null });
    const props = await mount(t, { inspection: offline("github_unreachable") });
    fireEvent.click(button("Try again"));
    await tick();
    expect(
      screen.getByText("This folder already has a remote, so there's nothing to publish.", { selector: "p" }),
    ).toBeTruthy();
    expect(screen.queryByText(/Zwiim will/)).toBeNull();
    expect(screen.queryByLabelText("Repository name")).toBeNull();
    // The remote may well be GitHub: the plain add keeps it, so the button doesn't say "without GitHub".
    expect(buttons()).toEqual(["Cancel", "Add repository"]);
    await settle();
    expect(checks(t)).toEqual([]);
    fireEvent.click(button("Add repository"));
    await tick();
    expect(adds(t)).toEqual([{ path: "/Users/me/my-app" }]);
    expect(props.onAdded).toHaveBeenCalledWith(added);
  });

  it("sends one add for a double Add without GitHub while the add is pending", async () => {
    const t = host();
    t.handlers["workspace.add"] = () => new Promise<WorkspaceInfo>(() => {});
    await mount(t, { inspection: offline("gh_unavailable") });
    await act(async () => {
      button("Add without GitHub").click();
      button("Add without GitHub").click();
    });
    await tick();
    expect(adds(t)).toHaveLength(1);
  });

  it("shows the large line when Add without GitHub finds the folder grew too large, and confirms it with the next click", async () => {
    const t = host();
    t.handlers["workspace.add"] = (p: { allow_large?: boolean }) => {
      if (!p.allow_large) throw new ZwiimClientError("large_folder", "big", 409, { files: 5001, bytes: 10, at_least: true });
      return added;
    };
    const props = await mount(t, { inspection: offline("gh_unavailable") });
    expect(screen.queryByText(/The first commit will hold/)).toBeNull();
    fireEvent.click(button("Add without GitHub"));
    await tick();
    expect(screen.getByText(text("The first commit will hold more than 5,000 files."), { selector: "p" })).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(adds(t)).toEqual([{ path: "/Users/me/my-app" }]);
    fireEvent.click(button("Add without GitHub"));
    await tick();
    expect(adds(t)).toEqual([{ path: "/Users/me/my-app" }, { path: "/Users/me/my-app", allow_large: true }]);
    expect(props.onAdded).toHaveBeenCalledWith(added);
  });

  it("drops a Try again answer that lands after Cancel closed the dialog", async () => {
    const t = host();
    let read: (i: WorkspaceInspection) => void = () => {};
    t.handlers["workspace.inspect"] = () => new Promise<WorkspaceInspection>((resolve) => (read = resolve));
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    const client = new HostClient({ transport: t });
    const controller = new HostController(client);
    client.start();
    await tick();
    const onClose = vi.fn();
    function Parent() {
      const [open, setOpen] = useState(true);
      return open ? (
        <PublishDialog
          subject={{ kind: "path", path: "/Users/me/my-app" }}
          inspection={offline("gh_unavailable")}
          remote={false}
          onClose={() => {
            onClose();
            setOpen(false);
          }}
          onAdded={vi.fn()}
        />
      ) : null;
    }
    render(
      <HostProvider value={controller}>
        <Parent />
      </HostProvider>,
    );
    await tick();
    fireEvent.click(button("Try again"));
    await tick();
    expect(inspects(t)).toHaveLength(1);
    fireEvent.click(button("Cancel"));
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("dialog")).toBeNull();
    await act(async () => read(plain()));
    await tick();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(adds(t)).toEqual([]);
    expect(checks(t)).toEqual([]);
    expect(errors).not.toHaveBeenCalled();
    errors.mockRestore();
  });

  it("adds without GitHub from the push failure", async () => {
    const t = host();
    t.handlers["workspace.add"] = (p: { publish?: unknown }) => {
      if (p.publish) throw new ZwiimClientError("publish_failed", "pushing failed", 422, { created_on_github: true });
      return added;
    };
    const props = await mount(t, { inspection: plain({ first_commit: large }) });
    await settle();
    fireEvent.click(confirm());
    await tick();
    fireEvent.click(button("Add without GitHub"));
    await tick();
    expect(adds(t)).toEqual([
      { path: "/Users/me/my-app", allow_large: true, publish: { owner: "Ashish1101", name: "my-app" } },
      { path: "/Users/me/my-app", allow_large: true },
    ]);
    expect(props.onAdded).toHaveBeenCalledWith(added);
  });

  it("marks the name taken when the create finds it already exists", async () => {
    const t = host();
    t.handlers["workspace.add"] = () => {
      throw new ZwiimClientError("already_exists", "A repository with this name already exists on GitHub", 409);
    };
    await mount(t);
    await settle();
    fireEvent.click(confirm());
    await tick();
    expect(screen.getByText(text("Ashish1101/my-app already exists."), { selector: "p" })).toBeTruthy();
    expect(nameField().disabled).toBe(false);
    expect(confirm().disabled).toBe(true);
    expect(screen.queryByRole("alert")).toBeNull();
  });
});

/** A first commit large enough to show its line. */
const big = { files: 12_431, bytes: 1.8 * 1024 ** 3, at_least: false, large: true };

/** A root listed before the add, under the same folder name as the one being added. */
const namesake: WorkspaceInfo = { ...added, workspace_id: "w0" };
const newRoot: WorkspaceInfo = { ...added, workspace_id: "w9" };

/** A Host whose publishing add outlasts the SDK's timeout (`code`), listing `before` until the add lands. */
function slowHost(code: "timeout" | "unknown_outcome" = "timeout", before: WorkspaceInfo[] = []) {
  const t = host();
  t.handlers["workspace.list"] = () => before;
  t.handlers["workspace.add"] = () => {
    throw new ZwiimClientError(code, "workspace.add timed out");
  };
  t.handlers["workspace.inspect"] = () => plain();
  return t;
}

/** Click Create and let the add time out. */
async function createAndTimeOut() {
  await settle();
  fireEvent.click(confirm());
  await tick();
}

/** Let the next 10 s re-inspect fire and settle. */
async function nextPoll() {
  await act(async () => void vi.advanceTimersByTime(10_000));
  await tick();
}

const still = (host = "this Mac") => screen.queryByText(`Still working on ${host}…`);
const lists = (t: FakeTransport) => t.calls.filter((c) => c.op === "workspace.list").length;

/** The Host's feed never answers, so the list isn't loaded when Create is clicked. */
function unlisted(t: FakeTransport) {
  const open = t.openEvents.bind(t);
  t.openEvents = (handlers) => {
    const channel = open(handlers);
    return { ...channel, send: (m) => void (m.op !== "host.subscribe" && channel.send(m)) };
  };
}

describe("PublishDialog watching an add that outlasts the timeout", () => {
  it("shows Still working on this Mac… with a spinner and Cancel only, not a failure", async () => {
    for (const code of ["timeout", "unknown_outcome"] as const) {
      const t = slowHost(code);
      await mount(t);
      await createAndTimeOut();
      const line = still()!;
      expect(line).toBeTruthy();
      expect(line.querySelector(".animate-spin")).toBeTruthy();
      expect(buttons()).toEqual(["Cancel"]);
      expect(button("Cancel").disabled).toBe(false);
      expect(screen.queryByRole("alert")).toBeNull();
      expect(screen.queryByLabelText("Repository name")).toBeNull();
      cleanup();
      vi.useRealTimers();
    }
    await mount(slowHost(), { remote: true });
    await createAndTimeOut();
    expect(still("the host")).toBeTruthy();
  });

  it("re-inspects the same folder every 10 s, one read at a time", async () => {
    const t = slowHost();
    await mount(t);
    await createAndTimeOut();
    expect(inspects(t)).toEqual([]);
    await act(async () => void vi.advanceTimersByTime(9_999));
    await tick();
    expect(inspects(t)).toEqual([]);
    await act(async () => void vi.advanceTimersByTime(1));
    await tick();
    expect(inspects(t)).toEqual([{ path: "/Users/me/my-app" }]);
    await nextPoll();
    await nextPoll();
    expect(inspects(t)).toHaveLength(3);
    expect(still()).toBeTruthy();

    // A read that hasn't answered holds the next one back.
    let read: (i: WorkspaceInspection) => void = () => {};
    t.handlers["workspace.inspect"] = () => new Promise<WorkspaceInspection>((resolve) => (read = resolve));
    await nextPoll();
    await nextPoll();
    await nextPoll();
    expect(inspects(t)).toHaveLength(4);
    await act(async () => read(plain()));
    await tick();
    await nextPoll();
    expect(inspects(t)).toHaveLength(5);
  });

  it("once the folder has an origin, refreshes the list and hands back the root that wasn't listed before", async () => {
    const t = slowHost("timeout", [namesake]);
    const props = await mount(t);
    await createAndTimeOut();
    t.handlers["workspace.inspect"] = () => plain({ git: "origin", first_commit: null });
    // Another new root, with a different folder name, is not this one.
    t.handlers["workspace.list"] = () => [namesake, { ...added, workspace_id: "w8", name: "other" }, newRoot];
    const listed = lists(t);
    await nextPoll();
    expect(lists(t)).toBe(listed + 1);
    expect(props.onAdded).toHaveBeenCalledTimes(1);
    expect(props.onAdded).toHaveBeenCalledWith(newRoot);
    expect(still()).toBeNull();
    const preparing = screen.getByRole("button", { name: /Preparing workspace…/ }) as HTMLButtonElement;
    expect(preparing.disabled).toBe(true);
    expect(button("Cancel").disabled).toBe(true);
    // Watching stopped.
    await nextPoll();
    await nextPoll();
    expect(inspects(t)).toHaveLength(1);
    expect(adds(t)).toHaveLength(1);
  });

  it("keeps watching while the origin is there but the root isn't listed yet", async () => {
    const t = slowHost("timeout", [namesake]);
    const props = await mount(t);
    await createAndTimeOut();
    t.handlers["workspace.inspect"] = () => plain({ git: "origin", first_commit: null });
    await nextPoll();
    expect(props.onAdded).not.toHaveBeenCalled();
    expect(still()).toBeTruthy();
    t.handlers["workspace.list"] = () => [namesake, newRoot];
    await nextPoll();
    expect(inspects(t)).toHaveLength(2);
    expect(props.onAdded).toHaveBeenCalledWith(newRoot);
  });

  it("re-inspects a listed root by its id and hands back that root", async () => {
    const root: WorkspaceInfo = { ...added, workspace_id: "r1" };
    const t = slowHost("unknown_outcome", [root]);
    const props = await mount(t, { subject: { kind: "root", workspace_id: "r1" } });
    await createAndTimeOut();
    await nextPoll();
    expect(inspects(t)).toEqual([{ workspace_id: "r1" }]);
    t.handlers["workspace.inspect"] = () => plain({ git: "origin", first_commit: null });
    await nextPoll();
    expect(inspects(t)).toEqual([{ workspace_id: "r1" }, { workspace_id: "r1" }]);
    expect(props.onAdded).toHaveBeenCalledWith(root);
  });

  it("keeps watching through a read that fails", async () => {
    const t = slowHost();
    const props = await mount(t);
    await createAndTimeOut();
    t.handlers["workspace.inspect"] = () => {
      throw new ZwiimClientError("network", "offline");
    };
    await nextPoll();
    await nextPoll();
    expect(inspects(t)).toHaveLength(2);
    expect(still()).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
    t.handlers["workspace.inspect"] = () => plain({ git: "origin", first_commit: null });
    t.handlers["workspace.list"] = () => [newRoot];
    await nextPoll();
    expect(props.onAdded).toHaveBeenCalledWith(newRoot);
  });

  it("stops after 10 minutes: the Host didn't finish, with Cancel only", async () => {
    const t = slowHost();
    const props = await mount(t);
    await createAndTimeOut();
    for (let i = 0; i < 59; i++) await nextPoll();
    expect(still()).toBeTruthy();
    await nextPoll();
    expect(still()).toBeNull();
    expect(
      screen.getByText("This Mac didn't finish adding this folder. Check the sidebar, or try again."),
    ).toBeTruthy();
    expect(buttons()).toEqual(["Cancel"]);
    const read = inspects(t).length;
    expect(read).toBeLessThanOrEqual(60);
    await nextPoll();
    await nextPoll();
    expect(inspects(t)).toHaveLength(read);
    expect(vi.getTimerCount()).toBe(0);
    fireEvent.click(button("Cancel"));
    expect(props.onClose).toHaveBeenCalledTimes(1);
    cleanup();
    vi.useRealTimers();

    await mount(slowHost(), { remote: true });
    await createAndTimeOut();
    await act(async () => void vi.advanceTimersByTime(600_000));
    await tick();
    expect(
      screen.getByText("The host didn't finish adding this folder. Check the sidebar, or try again."),
    ).toBeTruthy();
  });

  it("Cancel closes the dialog and stops watching", async () => {
    const t = slowHost();
    const props = await mount(t);
    await createAndTimeOut();
    await nextPoll();
    fireEvent.click(button("Cancel"));
    expect(props.onClose).toHaveBeenCalledTimes(1);
    await nextPoll();
    await nextPoll();
    expect(inspects(t)).toHaveLength(1);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("Escape and a backdrop click close the dialog and stop watching", async () => {
    for (const shut of [
      () => fireEvent.keyDown(document, { key: "Escape" }),
      () => fireEvent.pointerDown(screen.getByRole("dialog").parentElement!),
    ]) {
      const t = slowHost();
      const props = await mount(t);
      await createAndTimeOut();
      await nextPoll();
      shut();
      expect(props.onClose).toHaveBeenCalledTimes(1);
      await nextPoll();
      await nextPoll();
      expect(inspects(t)).toHaveLength(1);
      expect(vi.getTimerCount()).toBe(0);
      cleanup();
      vi.useRealTimers();
    }
  });

  it("hands back the only new root when it registered under another name (a symlinked pick)", async () => {
    const t = slowHost("timeout", [namesake]);
    const props = await mount(t, { subject: { kind: "path", path: "/Users/me/link" } });
    await createAndTimeOut();
    t.handlers["workspace.inspect"] = () => plain({ git: "origin", first_commit: null });
    const real: WorkspaceInfo = { ...added, workspace_id: "w9", name: "real" };
    t.handlers["workspace.list"] = () => [namesake, real];
    await nextPoll();
    expect(props.onAdded).toHaveBeenCalledWith(real);
  });

  it("matches no other-named root when more than one root is new", async () => {
    const t = slowHost("timeout", [namesake]);
    const props = await mount(t, { subject: { kind: "path", path: "/Users/me/link" } });
    await createAndTimeOut();
    t.handlers["workspace.inspect"] = () => plain({ git: "origin", first_commit: null });
    t.handlers["workspace.list"] = () => [
      namesake,
      { ...added, workspace_id: "w8", name: "other" },
      { ...added, workspace_id: "w9", name: "real" },
    ];
    await nextPoll();
    expect(props.onAdded).not.toHaveBeenCalled();
    expect(still()).toBeTruthy();
  });

  it("loads the list before Create records it, so a root listed before is never taken for the new one", async () => {
    const t = slowHost("timeout", [namesake]);
    unlisted(t);
    const props = await mount(t);
    await createAndTimeOut();
    expect(lists(t)).toBe(1);
    expect(adds(t)).toHaveLength(1);
    t.handlers["workspace.inspect"] = () => plain({ git: "origin", first_commit: null });
    await nextPoll();
    expect(props.onAdded).not.toHaveBeenCalled();
    t.handlers["workspace.list"] = () => [namesake, newRoot];
    await nextPoll();
    expect(props.onAdded).toHaveBeenCalledWith(newRoot);
  });

  it("matches no root when the list can't be loaded at Create", async () => {
    const t = slowHost("timeout");
    unlisted(t);
    t.handlers["workspace.list"] = () => {
      throw new ZwiimClientError("network", "offline");
    };
    const props = await mount(t);
    await createAndTimeOut();
    expect(adds(t)).toHaveLength(1);
    expect(still()).toBeTruthy();
    t.handlers["workspace.inspect"] = () => plain({ git: "origin", first_commit: null });
    t.handlers["workspace.list"] = () => [newRoot];
    await nextPoll();
    await nextPoll();
    expect(props.onAdded).not.toHaveBeenCalled();
    expect(still()).toBeTruthy();
  });

  it("hides the steps and the large line while watching and once it didn't finish", async () => {
    const t = slowHost();
    t.handlers["workspace.inspect"] = () => plain({ first_commit: big });
    await mount(t, { inspection: plain({ first_commit: big }) });
    expect(screen.getByText(/Zwiim will/)).toBeTruthy();
    expect(screen.getByText(/The first commit will hold/)).toBeTruthy();
    await createAndTimeOut();
    expect(still()).toBeTruthy();
    expect(screen.queryByText(/Zwiim will/)).toBeNull();
    expect(screen.queryByText(/The first commit will hold/)).toBeNull();
    await act(async () => void vi.advanceTimersByTime(600_000));
    await tick();
    expect(screen.getByText(/didn't finish adding this folder/)).toBeTruthy();
    expect(screen.queryByText(/Zwiim will/)).toBeNull();
    expect(screen.queryByText(/The first commit will hold/)).toBeNull();
  });

  it("stops watching when the dialog goes away", async () => {
    const t = slowHost();
    await mount(t);
    await createAndTimeOut();
    cleanup();
    await nextPoll();
    expect(inspects(t)).toEqual([]);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("shows a timed-out Add without GitHub as an error, with the form unchanged", async () => {
    const t = slowHost();
    await mount(t, { inspection: offline("gh_unavailable") });
    fireEvent.click(button("Add without GitHub"));
    await tick();
    expect(screen.getByRole("alert").textContent).toBe(
      "The host did not answer in time. Check the current state before trying again.",
    );
    expect(buttons()).toEqual(["Cancel", "Add without GitHub", "Try again"]);
    expect(still()).toBeNull();
    await nextPoll();
    expect(inspects(t)).toEqual([]);
  });
});

const firstWorktree: WorkspaceInfo = {
  ...added,
  workspace_id: "wt1",
  name: "my-app-wt1",
  kind: "worktree",
  parent_workspace_id: "w1",
  branch: "zwiim/wt1",
  base_branch: "main",
  base_commit: "3f9e1a2".padEnd(40, "b"),
};

/** A Host that reads the picked folder as `inspection`; `publishable` false is a Host from before publishing. */
function sidebarHost(inspection: WorkspaceInspection, publishable = true) {
  const t = host();
  const capabilities = CLIENT_CAPABILITIES.filter((c) => publishable || c !== "workspace.publish");
  t.handlers["host.hello"] = () => hostHello({ capabilities });
  t.handlers["workspace.inspect"] = () => inspection;
  t.handlers["workspace.create_worktree"] = () => firstWorktree;
  return t;
}

async function mountSidebar(t: FakeTransport, props: Partial<SidebarProps> = {}) {
  const client = new HostClient({ transport: t });
  const controller = new HostController(client);
  client.start();
  await tick();
  const all: SidebarProps = {
    workspaceId: null,
    onSelectWorkspace: vi.fn(),
    onSessionStarted: vi.fn(),
    pickFolder: vi.fn(async () => "/Users/me/my-app"),
    remote: false,
    ...props,
  };
  render(
    <HostProvider value={controller}>
      <Sidebar {...all} />
    </HostProvider>,
  );
  await tick();
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
  return all;
}

/** Click + and let the picked folder be read. */
async function pick() {
  openProject();
  await tick();
}

const ops = (t: FakeTransport, names: string[]) => t.calls.filter((c) => names.includes(c.op)).map((c) => c.op);
const worktrees = (t: FakeTransport) =>
  t.calls.filter((c) => c.op === "workspace.create_worktree").map((c) => c.params);
const dialog = () => screen.queryByRole("dialog", { name: "Create a private GitHub repo?" });

describe("Adding a folder from the sidebar", () => {
  it("reads the picked folder and asks before publishing a plain one: nothing is added yet (D11)", async () => {
    const t = sidebarHost(plain());
    await mountSidebar(t);
    await pick();
    expect(inspects(t)).toEqual([{ path: "/Users/me/my-app" }]);
    expect(dialog()).toBeTruthy();
    expect(button("Initialize Git and create repo")).toBeTruthy();
    expect(adds(t)).toEqual([]);

    fireEvent.click(button("Cancel"));
    expect(dialog()).toBeNull();
    expect(adds(t)).toEqual([]);
  });

  it("offers no second pick while the publish dialog is open", async () => {
    const t = sidebarHost(plain());
    const props = await mountSidebar(t);
    const add = screen.getByTitle("Add workspace") as HTMLButtonElement;
    await pick();
    expect(dialog()).toBeTruthy();
    expect(add.disabled).toBe(true);
    fireEvent.click(add);
    await tick();
    expect(props.pickFolder).toHaveBeenCalledTimes(1);
    expect(inspects(t)).toHaveLength(1);

    fireEvent.click(button("Cancel"));
    expect(add.disabled).toBe(false);
  });

  it("asks for a repository without an origin too: with history, unborn, or GitHub unavailable", async () => {
    const cases = [
      plain({ git: "no_origin", first_commit: null }),
      plain({ git: "no_origin" }),
      plain({ git: "no_origin", first_commit: null, github: { available: false, reason: "gh_unavailable" } }),
    ];
    for (const inspection of cases) {
      const t = sidebarHost(inspection);
      await mountSidebar(t);
      await pick();
      expect(dialog()).toBeTruthy();
      expect(adds(t)).toEqual([]);
      cleanup();
      vi.useRealTimers();
    }
    const t = sidebarHost(offline("gh_unavailable"));
    await mountSidebar(t);
    await pick();
    expect(screen.getByText("GitHub CLI isn't installed on this Mac.")).toBeTruthy();
    expect(buttons()).toContain("Add without GitHub");
  });

  it("names the host in the dialog when the Host is another computer", async () => {
    const t = sidebarHost(offline("gh_unavailable"));
    await mountSidebar(t, { remote: true });
    await pick();
    expect(screen.getByText("GitHub CLI isn't installed on the host.")).toBeTruthy();
  });

  it("adds a folder with a remote at once, then opens a new worktree in it", async () => {
    const t = sidebarHost(plain({ git: "origin", first_commit: null }));
    const props = await mountSidebar(t);
    // Folded away, the list unfolds so the new repository shows.
    fireEvent.click(screen.getByRole("button", { name: "Workspaces" }));
    expect(screen.queryByTestId("workspace-tree")).toBeNull();
    await pick();
    expect(dialog()).toBeNull();
    expect(ops(t, ["workspace.inspect", "workspace.add", "workspace.create_worktree"])).toEqual([
      "workspace.inspect",
      "workspace.add",
      "workspace.create_worktree",
    ]);
    expect(adds(t)).toEqual([{ path: "/Users/me/my-app" }]);
    expect(worktrees(t)).toEqual([{ workspace_id: "w1" }]);
    // The new worktree, never the root.
    expect(props.onSelectWorkspace).toHaveBeenCalledTimes(1);
    expect(props.onSelectWorkspace).toHaveBeenCalledWith("wt1");
    expect(screen.getByTestId("workspace-w1").contains(screen.getByTestId("workspace-wt1"))).toBe(true);
  });

  it("opens a new worktree after the dialog's add, preparing the workspace meanwhile", async () => {
    const t = sidebarHost(plain());
    let finish: (w: WorkspaceInfo) => void = () => {};
    t.handlers["workspace.create_worktree"] = () => new Promise<WorkspaceInfo>((resolve) => (finish = resolve));
    const props = await mountSidebar(t);
    await pick();
    await settle();
    fireEvent.click(confirm());
    await tick();
    expect(adds(t)).toEqual([{ path: "/Users/me/my-app", publish: { owner: "Ashish1101", name: "my-app" } }]);
    expect(worktrees(t)).toEqual([{ workspace_id: "w1" }]);
    const preparing = screen.getByRole("button", { name: /Preparing workspace…/ }) as HTMLButtonElement;
    expect(preparing.disabled).toBe(true);
    expect((button("Cancel") as HTMLButtonElement).disabled).toBe(true);
    expect(props.onSelectWorkspace).not.toHaveBeenCalled();

    await act(async () => finish(firstWorktree));
    await tick();
    expect(props.onSelectWorkspace).toHaveBeenCalledTimes(1);
    expect(props.onSelectWorkspace).toHaveBeenCalledWith("wt1");
    expect(dialog()).toBeNull();
    expect(screen.getByTestId("workspace-wt1")).toBeTruthy();
  });

  it("opens a new worktree in a repository whose add outlasted the timeout, once the watch finds it", async () => {
    const t = sidebarHost(plain());
    t.handlers["workspace.add"] = () => {
      throw new ZwiimClientError("timeout", "workspace.add timed out");
    };
    const props = await mountSidebar(t);
    await pick();
    await settle();
    fireEvent.click(confirm());
    await tick();
    expect(still()).toBeTruthy();
    t.handlers["workspace.inspect"] = () => plain({ git: "origin", first_commit: null });
    t.handlers["workspace.list"] = () => [added];
    await nextPoll();
    expect(worktrees(t)).toEqual([{ workspace_id: "w1" }]);
    expect(props.onSelectWorkspace).toHaveBeenCalledWith("wt1");
    expect(dialog()).toBeNull();
    expect(screen.getByTestId("workspace-w1").contains(screen.getByTestId("workspace-wt1"))).toBe(true);
  });

  it("unfolds the list for a repository added from the dialog", async () => {
    const t = sidebarHost(plain());
    await mountSidebar(t);
    fireEvent.click(screen.getByRole("button", { name: "Workspaces" }));
    expect(screen.queryByTestId("workspace-tree")).toBeNull();
    await pick();
    await settle();
    fireEvent.click(confirm());
    await tick();
    expect(dialog()).toBeNull();
    expect(screen.getByTestId("workspace-w1").contains(screen.getByTestId("workspace-wt1"))).toBe(true);
  });

  it("keeps the repository when its first worktree fails: the dialog closes and the error shows", async () => {
    const t = sidebarHost(plain());
    t.handlers["workspace.create_worktree"] = () => {
      throw new ZwiimClientError("internal", "worktree failed", 500);
    };
    const props = await mountSidebar(t);
    await pick();
    await settle();
    fireEvent.click(confirm());
    await tick();
    expect(dialog()).toBeNull();
    expect(screen.getByText("Something went wrong on the host: worktree failed")).toBeTruthy();
    expect(screen.getByTestId("workspace-w1")).toBeTruthy();
    expect(props.onSelectWorkspace).not.toHaveBeenCalled();
  });

  it("keeps a folder with a remote when its first worktree fails, and shows the error", async () => {
    const t = sidebarHost(plain({ git: "origin", first_commit: null }));
    t.handlers["workspace.create_worktree"] = () => {
      throw new ZwiimClientError("internal", "worktree failed", 500);
    };
    const props = await mountSidebar(t);
    await pick();
    expect(screen.getByText("Something went wrong on the host: worktree failed")).toBeTruthy();
    expect(screen.getByTestId("workspace-w1")).toBeTruthy();
    expect(props.onSelectWorkspace).not.toHaveBeenCalled();
  });

  it("shows a failed read of the folder in the sidebar, and adds nothing", async () => {
    const t = sidebarHost(plain());
    t.handlers["workspace.inspect"] = () => {
      throw new ZwiimClientError("not_found", "no such folder", 404);
    };
    await mountSidebar(t);
    await pick();
    expect(dialog()).toBeNull();
    expect(screen.getByText("no such folder")).toBeTruthy();
    expect(adds(t)).toEqual([]);
    expect((screen.getByTitle("Add workspace") as HTMLButtonElement).disabled).toBe(false);
  });

  it("adds at once on a Host without publishing, selecting the root as before; no inspect", async () => {
    const t = sidebarHost(plain(), false);
    const props = await mountSidebar(t);
    await pick();
    expect(dialog()).toBeNull();
    expect(adds(t)).toEqual([{ path: "/Users/me/my-app" }]);
    expect(props.onSelectWorkspace).toHaveBeenCalledWith("w1");
    expect(inspects(t)).toEqual([]);
    expect(worktrees(t)).toEqual([]);
  });

  it("keeps the large-folder card on a Host without publishing", async () => {
    const t = sidebarHost(plain(), false);
    t.handlers["workspace.add"] = (p: { allow_large?: boolean }) => {
      if (!p.allow_large) throw new ZwiimClientError("large_folder", "big", 409, { files: 5001, bytes: 10, at_least: true });
      return added;
    };
    const props = await mountSidebar(t);
    await pick();
    const card = screen.getByRole("alertdialog", { name: "Large folder" });
    fireEvent.click(screen.getByRole("button", { name: "Add anyway" }));
    await tick();
    expect(card.isConnected).toBe(false);
    expect(adds(t)).toEqual([{ path: "/Users/me/my-app" }, { path: "/Users/me/my-app", allow_large: true }]);
    expect(props.onSelectWorkspace).toHaveBeenCalledWith("w1");
    expect(inspects(t)).toEqual([]);
  });
});

const session = (session_id: string, updated_at: string): SessionSummary => ({
  session_id,
  workspace_id: null,
  agent: "codex",
  title: `task ${session_id}`,
  status: "completed",
  live: false,
  created_at: "2026-10-01T00:00:00Z",
  updated_at,
});

/** A Host listing root `w1` with `worktrees` and their `sessions`; it reads the root as `inspection`. */
function rootHost(
  inspection: WorkspaceInspection,
  worktrees: WorkspaceInfo[] = [],
  sessions: Record<string, SessionSummary[]> = {},
  publishable = true,
) {
  const t = sidebarHost(inspection, publishable);
  t.handlers["workspace.list"] = () => [added, ...worktrees];
  t.handlers["session.list"] = (p: { workspace_id: string }) => sessions[p.workspace_id] ?? [];
  return t;
}

const worktreeOf = (id: string, extra: Partial<WorkspaceInfo> = {}): WorkspaceInfo => ({
  ...firstWorktree,
  workspace_id: id,
  name: `my-app-${id}`,
  branch: `zwiim/${id}`,
  ...extra,
});

/** Click root `w1`'s row and let what it starts land. */
async function openRoot() {
  fireEvent.click(screen.getByText("my-app"));
  await tick();
}

const rootRow = () => screen.getByTestId("workspace-w1").firstElementChild as HTMLElement;

describe("Opening a repository from the sidebar", () => {
  it("opens the live worktree whose newest session was prompted last, sending nothing", async () => {
    const t = rootHost(
      plain({ git: "origin", first_commit: null }),
      [worktreeOf("wt1"), worktreeOf("wt2"), worktreeOf("gone", { removed: true })],
      {
        wt1: [session("s1", "2026-10-05T10:00:00Z")],
        wt2: [session("s2", "2026-10-06T10:00:00Z")],
        gone: [session("s3", "2026-10-07T10:00:00Z")],
      },
    );
    const props = await mountSidebar(t);
    await openRoot();
    expect(props.onSelectWorkspace).toHaveBeenCalledTimes(1);
    expect(props.onSelectWorkspace).toHaveBeenCalledWith("wt2");
    expect(ops(t, ["workspace.inspect", "workspace.add", "workspace.create_worktree"])).toEqual([]);
  });

  it("reads a repository with no live worktree, then creates one and selects it, preparing meanwhile", async () => {
    const t = rootHost(plain({ git: "origin", first_commit: null }), [worktreeOf("gone", { removed: true })]);
    let finish: (w: WorkspaceInfo) => void = () => {};
    t.handlers["workspace.create_worktree"] = () => new Promise<WorkspaceInfo>((resolve) => (finish = resolve));
    const props = await mountSidebar(t);
    expect(rootRow().textContent).not.toContain("Preparing workspace…");
    await openRoot();
    expect(inspects(t)).toEqual([{ workspace_id: "w1" }]);
    expect(worktrees(t)).toEqual([{ workspace_id: "w1" }]);
    expect(rootRow().textContent).toContain("Preparing workspace…");
    expect(props.onSelectWorkspace).not.toHaveBeenCalled();

    await act(async () => finish(firstWorktree));
    await tick();
    expect(props.onSelectWorkspace).toHaveBeenCalledTimes(1);
    expect(props.onSelectWorkspace).toHaveBeenCalledWith("wt1");
    expect(rootRow().textContent).not.toContain("Preparing workspace…");
    expect(dialog()).toBeNull();
    expect(adds(t)).toEqual([]);
    expect(screen.getByTestId("workspace-w1").contains(screen.getByTestId("workspace-wt1"))).toBe(true);
  });

  it("shows Preparing workspace… while the repository is read", async () => {
    const t = rootHost(plain());
    let read: (i: WorkspaceInspection) => void = () => {};
    t.handlers["workspace.inspect"] = () => new Promise<WorkspaceInspection>((resolve) => (read = resolve));
    await mountSidebar(t);
    await openRoot();
    expect(rootRow().textContent).toContain("Preparing workspace…");
    expect(dialog()).toBeNull();

    await act(async () => read(plain()));
    await tick();
    // The dialog takes over from the row.
    expect(dialog()).toBeTruthy();
    expect(rootRow().textContent).not.toContain("Preparing workspace…");
  });

  it("creates a worktree at once for a repository with history and no remote", async () => {
    const t = rootHost(plain({ git: "no_origin", first_commit: null }));
    const props = await mountSidebar(t);
    await openRoot();
    expect(dialog()).toBeNull();
    expect(worktrees(t)).toEqual([{ workspace_id: "w1" }]);
    expect(props.onSelectWorkspace).toHaveBeenCalledWith("wt1");
  });

  it("asks before publishing a plain or unborn repository, then adds it by id and opens a worktree", async () => {
    for (const inspection of [plain(), plain({ git: "no_origin" })]) {
      const t = rootHost(inspection);
      const props = await mountSidebar(t);
      await openRoot();
      expect(inspects(t)).toEqual([{ workspace_id: "w1" }]);
      expect(dialog()).toBeTruthy();
      expect(adds(t)).toEqual([]);
      expect(worktrees(t)).toEqual([]);

      await settle();
      fireEvent.click(confirm());
      await tick();
      expect(adds(t)).toEqual([{ workspace_id: "w1", publish: { owner: "Ashish1101", name: "my-app" } }]);
      expect(worktrees(t)).toEqual([{ workspace_id: "w1" }]);
      expect(props.onSelectWorkspace).toHaveBeenCalledTimes(1);
      expect(props.onSelectWorkspace).toHaveBeenCalledWith("wt1");
      expect(dialog()).toBeNull();
      cleanup();
      vi.useRealTimers();
    }
  });

  it("shows the unavailable form, and the large line of a too-large plain folder", async () => {
    let t = rootHost(offline("gh_unauthenticated"));
    await mountSidebar(t);
    await openRoot();
    expect(
      screen.getByText(text("GitHub CLI isn't signed in on this Mac. Run gh auth login, then try again."), { selector: "p" }),
    ).toBeTruthy();
    expect(buttons()).toContain("Add without GitHub");
    cleanup();
    vi.useRealTimers();

    t = rootHost(plain({ first_commit: large }));
    await mountSidebar(t);
    await openRoot();
    expect(dialog()).toBeTruthy();
    expect(dialog()!.textContent).toContain("12,431 files (1.8 GB)");
  });

  it("closes the dialog with nothing added when it is cancelled", async () => {
    const t = rootHost(plain());
    const props = await mountSidebar(t);
    await openRoot();
    fireEvent.click(button("Cancel"));
    expect(dialog()).toBeNull();
    expect(adds(t)).toEqual([]);
    expect(worktrees(t)).toEqual([]);
    expect(props.onSelectWorkspace).not.toHaveBeenCalled();
  });

  it("reads the repository once for a double click, and not again while the dialog is open", async () => {
    const t = rootHost(plain());
    let read: (i: WorkspaceInspection) => void = () => {};
    t.handlers["workspace.inspect"] = () => new Promise<WorkspaceInspection>((resolve) => (read = resolve));
    await mountSidebar(t);
    await openRoot();
    await openRoot();
    expect(inspects(t)).toHaveLength(1);
    // Nor can + start a second read meanwhile.
    expect((screen.getByTitle("Add workspace") as HTMLButtonElement).disabled).toBe(true);

    await act(async () => read(plain()));
    await tick();
    expect(dialog()).toBeTruthy();
    await openRoot();
    expect(inspects(t)).toHaveLength(1);
    expect(screen.getAllByRole("dialog")).toHaveLength(1);
  });

  it("reads no repository while the folder picker is open, and again once it is cancelled", async () => {
    const t = rootHost(plain());
    let picked: (p: string | null) => void = () => {};
    await mountSidebar(t, { pickFolder: vi.fn(() => new Promise<string | null>((resolve) => (picked = resolve))) });
    const add = screen.getByTitle("Add workspace") as HTMLButtonElement;
    openProject();
    await tick();
    expect(add.disabled).toBe(true);
    await openRoot();
    expect(inspects(t)).toEqual([]);
    expect(dialog()).toBeNull();

    await act(async () => picked(null));
    await tick();
    expect(add.disabled).toBe(false);
    await openRoot();
    expect(inspects(t)).toEqual([{ workspace_id: "w1" }]);
  });

  it("sends nothing for a repository without a worktree while the Host is offline, but still selects one with", async () => {
    const t = rootHost(plain());
    t.handlers["workspace.list"] = () => [added, { ...added, workspace_id: "w2", name: "other" }, worktreeOf("wt2", { parent_workspace_id: "w2" })];
    const props = await mountSidebar(t);
    await act(async () => t.handlersForEvents!.onClose({ code: 1006, reason: "" }));
    await tick();
    await openRoot();
    expect(inspects(t)).toEqual([]);
    expect(worktrees(t)).toEqual([]);
    expect(rootRow().textContent).not.toContain("Preparing workspace…");
    expect(props.onSelectWorkspace).not.toHaveBeenCalled();

    fireEvent.click(screen.getByText("other"));
    expect(props.onSelectWorkspace).toHaveBeenCalledWith("wt2");
  });

  it("keeps the dialog's form and shows its error when a root's add fails", async () => {
    const t = rootHost(plain());
    t.handlers["workspace.add"] = () => {
      throw new Error("Folder is gone");
    };
    const props = await mountSidebar(t);
    await openRoot();
    await settle();
    fireEvent.click(confirm());
    await tick();
    expect(adds(t)).toEqual([{ workspace_id: "w1", publish: { owner: "Ashish1101", name: "my-app" } }]);
    expect(screen.getByRole("alert").textContent).toBe("Folder is gone");
    expect(dialog()).toBeTruthy();
    expect(nameField().disabled).toBe(false);
    expect(nameField().value).toBe("my-app");
    expect(confirm().disabled).toBe(false);
    expect(worktrees(t)).toEqual([]);
    expect(props.onSelectWorkspace).not.toHaveBeenCalled();
  });

  it("shows a failed read or worktree in the sidebar's error line, selecting nothing", async () => {
    let t = rootHost(plain());
    t.handlers["workspace.inspect"] = () => {
      throw new ZwiimClientError("not_found", "no such folder", 404);
    };
    let props = await mountSidebar(t);
    await openRoot();
    expect(screen.getByText("no such folder")).toBeTruthy();
    expect(dialog()).toBeNull();
    expect(props.onSelectWorkspace).not.toHaveBeenCalled();
    expect(rootRow().textContent).not.toContain("Preparing workspace…");
    cleanup();
    vi.useRealTimers();

    t = rootHost(plain({ git: "origin", first_commit: null }));
    t.handlers["workspace.create_worktree"] = () => {
      throw new ZwiimClientError("internal", "worktree failed", 500);
    };
    props = await mountSidebar(t);
    await openRoot();
    expect(screen.getByText("Something went wrong on the host: worktree failed")).toBeTruthy();
    expect(props.onSelectWorkspace).not.toHaveBeenCalled();
    expect(rootRow().textContent).not.toContain("Preparing workspace…");
  });

  it("keeps New worktree on the repository's row", async () => {
    const t = rootHost(plain({ git: "origin", first_commit: null }));
    await mountSidebar(t);
    expect(rootRow().querySelector('[title="New worktree"]')).toBeTruthy();
  });

  it("selects the root on a Host without publishing, as before; nothing is read", async () => {
    const t = rootHost(plain(), [worktreeOf("wt2")], {}, false);
    const props = await mountSidebar(t);
    await openRoot();
    expect(props.onSelectWorkspace).toHaveBeenCalledTimes(1);
    expect(props.onSelectWorkspace).toHaveBeenCalledWith("w1");
    expect(inspects(t)).toEqual([]);
    expect(worktrees(t)).toEqual([]);
  });
});
