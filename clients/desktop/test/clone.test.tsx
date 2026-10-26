import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  CLIENT_CAPABILITIES,
  HostClient,
  HostController,
  ZwiimClientError,
  type CloneJob,
  type GithubRepo,
  type WorkspaceInfo,
} from "@zwiim/sdk";
import { HostProvider } from "@/lib/host-context";
import { GithubCloneDialog, type GithubCloneDialogProps } from "@/panels/GithubCloneDialog";
import { Sidebar, type SidebarProps } from "@/panels/Sidebar";
import { FakeTransport, hostHello, tick } from "./fakes";

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

const HOUR = 3600_000;

const repo = (owner: string, name: string, extra: Partial<GithubRepo> = {}): GithubRepo => ({
  owner,
  name,
  private: false,
  fork: false,
  archived: false,
  description: null,
  pushed_at: null,
  size_kb: 10,
  default_branch: "main",
  workspace_id: null,
  ...extra,
});

const job = (extra: Partial<CloneJob> = {}): CloneJob => ({
  clone_id: "c1",
  owner: "acme",
  name: "web",
  state: "cloning",
  phase: "receiving",
  percent: 42,
  workspace_id: null,
  worktree_id: null,
  error: null,
  ...extra,
});

const firstPage = () => [
  repo("acme", "web", { private: true, description: "The web app", pushed_at: new Date(Date.now() - 2 * HOUR).toISOString() }),
  repo("acme", "api", { fork: true }),
  repo("me", "old", { archived: true, workspace_id: "w9" }),
];

/** A Host whose `github.repos` serves `pages` (page 1 first). */
function githubHost(pages: GithubRepo[][] = [firstPage()]) {
  const t = new FakeTransport();
  t.handlers["github.repos"] = (p: { page?: number }) => {
    const i = (p.page ?? 1) - 1;
    return { repos: pages[i] ?? [], next_page: i + 1 < pages.length ? i + 2 : null };
  };
  t.handlers["workspace.clone_cancel"] = () => ({ cancelled: true });
  return t;
}

/** A Host whose `github.repos` always fails with `e`. */
function failing(e: ZwiimClientError) {
  const t = githubHost();
  t.handlers["github.repos"] = () => {
    throw e;
  };
  return t;
}

async function mountDialog(t: FakeTransport, props: Partial<GithubCloneDialogProps> = {}) {
  const client = new HostClient({ transport: t });
  const controller = new HostController(client);
  client.start();
  await tick();
  const all: GithubCloneDialogProps = {
    remote: false,
    jobs: [],
    onClose: vi.fn(),
    onClone: vi.fn(async () => {}),
    ...props,
  };
  const view = render(
    <HostProvider value={controller}>
      <GithubCloneDialog {...all} />
    </HostProvider>,
  );
  await tick();
  /** Render again with other jobs, as the Host's `clone.changed` would. */
  const jobs = (next: CloneJob[]) =>
    view.rerender(
      <HostProvider value={controller}>
        <GithubCloneDialog {...all} jobs={next} />
      </HostProvider>,
    );
  return { props: all, jobs };
}

const repoCalls = (t: FakeTransport) => t.calls.filter((c) => c.op === "github.repos").map((c) => c.params);
const row = (fullName: string) => within(screen.getByTestId(`repo-${fullName}`));
const rows = () => screen.queryAllByTestId(/^repo-/).map((r) => r.getAttribute("data-testid"));
const search = () => screen.getByLabelText("Search repositories") as HTMLInputElement;
const type = (text: string) => fireEvent.change(search(), { target: { value: text } });
const text = (s: string) => (_: string, el: Element | null) => el?.textContent === s;
const button = (name: string) => screen.getByRole("button", { name }) as HTMLButtonElement;

describe("GithubCloneDialog", () => {
  it("shows loading, then the account's repositories with their badges; the search is focused", async () => {
    const t = githubHost();
    let answer!: (v: unknown) => void;
    t.handlers["github.repos"] = () => new Promise((r) => (answer = r));
    await mountDialog(t);
    expect(screen.getByRole("dialog", { name: "Open GitHub project" })).toBeTruthy();
    expect(screen.getByText("Loading repositories…")).toBeTruthy();
    expect(document.activeElement).toBe(search());
    expect(repoCalls(t)).toEqual([{}]);

    await act(async () => answer({ repos: firstPage(), next_page: null }));
    await tick();
    expect(screen.queryByText("Loading repositories…")).toBeNull();
    expect(rows()).toEqual(["repo-acme/web", "repo-acme/api", "repo-me/old"]);
    expect(row("acme/web").getByLabelText("Private")).toBeTruthy();
    expect(row("acme/web").getByText("The web app")).toBeTruthy();
    expect(row("acme/web").getByText("2 h ago")).toBeTruthy();
    expect(row("acme/api").getByText("Fork")).toBeTruthy();
    expect(row("acme/api").queryByLabelText("Private")).toBeNull();
    expect(row("me/old").getByText("Archived")).toBeTruthy();
    expect(row("me/old").getByText("Added")).toBeTruthy();
    expect(row("acme/web").queryByText("Added")).toBeNull();
    expect(screen.queryByRole("button", { name: "Load more" })).toBeNull();
  });

  it("filters loaded rows by name, description, or a pasted URL", async () => {
    await mountDialog(githubHost());
    type("api");
    expect(rows()).toEqual(["repo-acme/api"]);
    type("web app");
    expect(rows()).toEqual(["repo-acme/web"]);
    type("https://github.com/ACME/Web.git");
    expect(rows()).toEqual(["repo-acme/web"]);
    type("git@github.com:acme/web.git");
    expect(rows()).toEqual(["repo-acme/web"]);
    type("nothing-like-it");
    expect(rows()).toEqual([]);
    expect(screen.getByText("No repositories match.")).toBeTruthy();
  });

  it("offers a pasted repository no page lists, and clones it", async () => {
    const { props } = await mountDialog(githubHost());
    type("https://github.com/acme/secret/tree/main");
    expect(rows()).toEqual(["repo-acme/secret"]);
    fireEvent.click(button("Clone acme/secret"));
    await tick();
    expect(props.onClone).toHaveBeenCalledWith("acme", "secret");

    type("git@github.com:acme/secret.git/");
    expect(rows()).toEqual(["repo-acme/secret"]);
    expect(screen.queryByText("No repositories match.")).toBeNull();
  });

  it("loads more pages and appends them", async () => {
    const t = githubHost([firstPage(), [repo("acme", "docs")]]);
    await mountDialog(t);
    fireEvent.click(button("Load more"));
    await tick();
    expect(repoCalls(t)).toEqual([{}, { page: 2 }]);
    expect(rows()).toEqual(["repo-acme/web", "repo-acme/api", "repo-me/old", "repo-acme/docs"]);
    expect(screen.queryByRole("button", { name: "Load more" })).toBeNull();
  });

  it("a loadMore failure shows its error copy beside Load more", async () => {
    const t = githubHost([firstPage(), [repo("acme", "docs")]]);
    const list = t.handlers["github.repos"]!;
    await mountDialog(t);
    t.handlers["github.repos"] = () => {
      throw new ZwiimClientError("network", "GitHub could not be reached", 502);
    };
    fireEvent.click(button("Load more"));
    await tick();
    expect(screen.getByText("Couldn't reach GitHub.")).toBeTruthy();
    expect(rows()).toEqual(["repo-acme/web", "repo-acme/api", "repo-me/old"]);

    t.handlers["github.repos"] = list;
    fireEvent.click(button("Load more"));
    await tick();
    expect(rows()).toEqual(["repo-acme/web", "repo-acme/api", "repo-me/old", "repo-acme/docs"]);
  });

  it("a loadMore the signed-out GitHub CLI refused offers Sign in to GitHub, then loads the page", async () => {
    const t = githubHost([firstPage(), [repo("acme", "docs")]]);
    const list = t.handlers["github.repos"]!;
    let signedIn = false;
    await mountDialog(t);
    t.handlers["github.repos"] = (p: { page?: number }) => {
      if (!signedIn) throw new ZwiimClientError("gh_unauthenticated", "not logged in", 422);
      return list(p);
    };
    t.handlers["github.sign_in"] = () => {
      signedIn = true;
      return { state: "signed_in", login: "Ashish1101" };
    };
    fireEvent.click(button("Load more"));
    await tick();
    expect(screen.getByText(text("GitHub CLI isn't signed in on this Mac."))).toBeTruthy();
    fireEvent.click(button("Sign in to GitHub"));
    await tick();
    expect(rows()).toEqual(["repo-acme/web", "repo-acme/api", "repo-me/old", "repo-acme/docs"]);
    expect(screen.queryByText(/isn't signed in/)).toBeNull();
    expect(screen.queryByRole("button", { name: "Sign in to GitHub" })).toBeNull();
  });

  it("a Host's GitHub failure offers Try again, which refreshes", async () => {
    const t = githubHost();
    const list = t.handlers["github.repos"]!;
    t.handlers["github.repos"] = () => {
      throw new ZwiimClientError("network", "GitHub could not be reached", 502);
    };
    await mountDialog(t);
    expect(screen.getByText("Couldn't reach GitHub.")).toBeTruthy();
    expect(rows()).toEqual([]);

    t.handlers["github.repos"] = list;
    fireEvent.click(button("Try again"));
    await tick();
    expect(repoCalls(t)).toEqual([{}, { refresh: true }]);
    expect(rows()).toEqual(["repo-acme/web", "repo-acme/api", "repo-me/old"]);
  });

  it("tells a Host's GitHub failure from the connection's own", async () => {
    await mountDialog(failing(new ZwiimClientError("timeout", "GitHub did not answer", 504)));
    expect(screen.getByText("Couldn't reach GitHub.")).toBeTruthy();
    cleanup();

    await mountDialog(failing(new ZwiimClientError("network", "Transport closed")));
    expect(screen.getByText(/^Cannot reach the host/)).toBeTruthy();
    cleanup();

    await mountDialog(failing(new ZwiimClientError("auth_failed", "refused", 401)));
    expect(screen.getByText(text("GitHub refused the credentials. Check gh auth status."))).toBeTruthy();
  });

  it("without GitHub CLI shows PublishDialog's line and no list", async () => {
    await mountDialog(failing(new ZwiimClientError("gh_unauthenticated", "not logged in", 422)), { remote: true });
    expect(screen.getByText(text("GitHub CLI isn't signed in on the host."))).toBeTruthy();
    expect(screen.queryByLabelText("Search repositories")).toBeNull();
    expect(button("Try again")).toBeTruthy();
    expect(button("Sign in to GitHub")).toBeTruthy();
    cleanup();

    await mountDialog(failing(new ZwiimClientError("gh_unavailable", "not installed", 422)));
    expect(screen.getByText(text("GitHub CLI isn't installed on this Mac."))).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Sign in to GitHub" })).toBeNull();
  });

  it("signs the Host in to GitHub when the list says it's signed out, then lists", async () => {
    const t = githubHost();
    let signedIn = false;
    const repos = t.handlers["github.repos"]!;
    t.handlers["github.repos"] = (p: { page?: number }) => {
      if (!signedIn) throw new ZwiimClientError("gh_unauthenticated", "not logged in", 422);
      return repos(p);
    };
    t.handlers["github.sign_in"] = () => {
      signedIn = true;
      return { state: "signed_in", login: "Ashish1101" };
    };
    await mountDialog(t);
    fireEvent.click(button("Sign in to GitHub"));
    await tick();
    expect(screen.getByLabelText("Search repositories")).toBeTruthy();
    expect(screen.queryByText(/isn't signed in/)).toBeNull();
  });

  it("on a Host without GitHub sign-in, still says to run gh auth login", async () => {
    const t = failing(new ZwiimClientError("gh_unauthenticated", "not logged in", 422));
    t.handlers["host.hello"] = () =>
      hostHello({ capabilities: CLIENT_CAPABILITIES.filter((c) => c !== "github.auth") });
    await mountDialog(t, { remote: true });
    expect(
      screen.getByText(text("GitHub CLI isn't signed in on the host. Run gh auth login, then try again.")),
    ).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Sign in to GitHub" })).toBeNull();
    cleanup();

    await mountDialog(failing(new ZwiimClientError("gh_unavailable", "not installed", 422)));
    expect(screen.getByText(text("GitHub CLI isn't installed on this Mac."))).toBeTruthy();
  });

  it("a refused clone shows the Host's reason on its row", async () => {
    const onClone = vi.fn(async () => {
      throw new ZwiimClientError("invalid_repository", "That is not a repository name", 422);
    });
    await mountDialog(githubHost(), { onClone });
    fireEvent.click(button("Clone acme/api"));
    await tick();
    expect(row("acme/api").getByText("That is not a repository name")).toBeTruthy();
    expect(button("Clone acme/api").disabled).toBe(false);
  });

  it("a clone in flight shows its phase and progress, and Cancel stops it", async () => {
    const t = githubHost();
    const { jobs } = await mountDialog(t, { jobs: [job()] });
    expect(row("acme/web").getByText("Receiving objects 42%")).toBeTruthy();
    expect(row("acme/web").getByRole("progressbar").getAttribute("aria-valuenow")).toBe("42");
    expect(button("Clone acme/web").disabled).toBe(true);
    // Other rows stay clickable: the Host queues them.
    expect(button("Clone acme/api").disabled).toBe(false);
    fireEvent.click(row("acme/web").getByRole("button", { name: "Cancel" }));
    await tick();
    expect(t.calls.filter((c) => c.op === "workspace.clone_cancel").map((c) => c.params)).toEqual([{ clone_id: "c1" }]);

    jobs([job({ state: "queued", phase: null, percent: null })]);
    expect(row("acme/web").getByText("Waiting to clone…")).toBeTruthy();
    jobs([job({ phase: "resolving", percent: 7 })]);
    expect(row("acme/web").getByText("Resolving deltas 7%")).toBeTruthy();
    jobs([job({ phase: "checkout", percent: 99 })]);
    expect(row("acme/web").getByText("Checking out files 99%")).toBeTruthy();
    jobs([job({ state: "adding", phase: null, percent: null })]);
    expect(row("acme/web").getByText("Opening…")).toBeTruthy();
    expect((row("acme/web").getByRole("button", { name: "Cancel" }) as HTMLButtonElement).disabled).toBe(true);
    jobs([job({ state: "done", workspace_id: "w1", worktree_id: "w2" })]);
    expect(row("acme/web").getByText("Added")).toBeTruthy();
    expect(row("acme/web").queryByRole("progressbar")).toBeNull();
  });

  it("a failed clone shows the §5 copy and Retry", async () => {
    const { props, jobs } = await mountDialog(githubHost());
    const failed = (code: string, message = "Host message") => jobs([job({ state: "failed", error: { code, message } })]);
    const line = (s: string) => row("acme/web").getByText(text(s));

    failed("network");
    expect(line("Couldn't reach GitHub.")).toBeTruthy();
    failed("not_found");
    expect(line("acme/web was not found, or this account can't see it.")).toBeTruthy();
    failed("auth_failed");
    expect(line("GitHub refused the credentials. Check gh auth status.")).toBeTruthy();
    failed("timeout");
    expect(line("The clone stopped making progress.")).toBeTruthy();
    failed("failed");
    expect(line("The clone failed.")).toBeTruthy();
    failed("gh_unavailable");
    expect(line("GitHub CLI isn't installed on this Mac.")).toBeTruthy();
    failed("gh_unauthenticated");
    expect(line("GitHub CLI isn't signed in on this Mac.")).toBeTruthy();
    expect(row("acme/web").getByRole("button", { name: "Sign in to GitHub" })).toBeTruthy();
    // A Zwiim add or worktree failure keeps its own Zwiim copy.
    failed("worktree_failed", "The worktree could not be created");
    expect(line("The worktree could not be created")).toBeTruthy();

    fireEvent.click(row("acme/web").getByRole("button", { name: "Retry" }));
    await tick();
    expect(props.onClone).toHaveBeenCalledWith("acme", "web");
  });

  it("offline, nothing can be cloned or cancelled", async () => {
    const t = githubHost();
    await mountDialog(t, { jobs: [job()] });
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    await act(async () => t.handlersForEvents!.onClose({ code: 1006, reason: "" }));
    await tick();
    expect(button("Clone acme/api").disabled).toBe(true);
    expect((row("acme/web").getByRole("button", { name: "Cancel" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("Escape, the backdrop and Close close it", async () => {
    const { props } = await mountDialog(githubHost());
    fireEvent.keyDown(document, { key: "Escape" });
    fireEvent.pointerDown(screen.getByRole("dialog").parentElement!);
    fireEvent.click(button("Close"));
    expect(props.onClose).toHaveBeenCalledTimes(3);
  });
});

const ws = (id: string, extra: Partial<WorkspaceInfo> = {}): WorkspaceInfo => ({
  workspace_id: id,
  name: "web",
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

/** A Host that lists GitHub repositories and starts clones as `c1`, `c2`, … */
function cloneHost() {
  const t = githubHost();
  let n = 0;
  t.handlers["workspace.clone"] = () => ({ clone_id: `c${++n}` });
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
    pickFolder: vi.fn(async () => null),
    remote: false,
    ...props,
  };
  render(
    <HostProvider value={controller}>
      <Sidebar {...all} />
    </HostProvider>,
  );
  await tick();
  return all;
}

const plusMenu = () => screen.queryByRole("menu", { name: "Add workspace" });
const githubDialog = () => screen.queryByRole("dialog", { name: "Open GitHub project" });

async function openGithub() {
  fireEvent.click(screen.getByTitle("Add workspace"));
  fireEvent.click(within(plusMenu()!).getByRole("menuitem", { name: "Open GitHub project" }));
  await tick();
}

describe("The Workspaces + menu", () => {
  it("offers Open project and Open GitHub project", async () => {
    const t = cloneHost();
    const props = await mountSidebar(t);
    fireEvent.click(screen.getByTitle("Add workspace"));
    const items = within(plusMenu()!).getAllByRole("menuitem").map((i) => i.textContent);
    expect(items).toEqual(["Open project", "Open GitHub project"]);

    fireEvent.click(within(plusMenu()!).getByRole("menuitem", { name: "Open project" }));
    await tick();
    expect(props.pickFolder).toHaveBeenCalledTimes(1);
    expect(plusMenu()).toBeNull();

    await openGithub();
    expect(plusMenu()).toBeNull();
    expect(githubDialog()).toBeTruthy();
    expect(t.calls.some((c) => c.op === "github.repos")).toBe(true);
  });

  it("picks a folder at once on a Host that cannot clone", async () => {
    const t = cloneHost();
    t.handlers["host.hello"] = () =>
      hostHello({ capabilities: CLIENT_CAPABILITIES.filter((c) => c !== "workspace.clone") });
    const props = await mountSidebar(t);
    fireEvent.click(screen.getByTitle("Add workspace"));
    await tick();
    expect(plusMenu()).toBeNull();
    expect(props.pickFolder).toHaveBeenCalledTimes(1);
  });

  it("closes on Escape and on a click elsewhere", async () => {
    await mountSidebar(cloneHost());
    fireEvent.click(screen.getByTitle("Add workspace"));
    expect(plusMenu()).toBeTruthy();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(plusMenu()).toBeNull();
    fireEvent.click(screen.getByTitle("Add workspace"));
    expect(plusMenu()).toBeTruthy();
    fireEvent.pointerDown(document.body);
    expect(plusMenu()).toBeNull();
  });

  it("a clone started while the list is folded still shows its row", async () => {
    const t = cloneHost();
    await mountSidebar(t);
    fireEvent.click(screen.getByRole("button", { name: "Workspaces" }));
    expect(screen.queryByTestId("workspace-tree")).toBeNull();
    await openGithub();
    fireEvent.click(screen.getByRole("button", { name: "Clone acme/web" }));
    await tick();
    await t.feedEvent("clone.changed", { clone: job() });
    expect(screen.getByTestId("clone-c1")).toBeTruthy();
  });

  it("selects the worktree of a clone this window started once it is done, and closes the dialog", async () => {
    const t = cloneHost();
    const props = await mountSidebar(t);
    await openGithub();
    fireEvent.click(screen.getByRole("button", { name: "Clone acme/web" }));
    await tick();
    expect(t.calls.filter((c) => c.op === "workspace.clone").map((c) => c.params)).toEqual([
      { owner: "acme", name: "web" },
    ]);

    await t.feedEvent("clone.changed", { clone: job({ state: "queued", phase: null, percent: null }) });
    await t.feedEvent("clone.changed", { clone: job() });
    expect(githubDialog()).toBeTruthy();
    expect(props.onSelectWorkspace).not.toHaveBeenCalled();

    await t.feedEvent("clone.changed", { clone: job({ state: "done", workspace_id: "w1", worktree_id: "w2" }) });
    expect(props.onSelectWorkspace).toHaveBeenCalledWith("w2");
    expect(githubDialog()).toBeNull();

    // Seen again (a replay), it selects nothing more.
    await t.feedEvent("clone.changed", { clone: job({ state: "done", workspace_id: "w1", worktree_id: "w2" }) });
    expect(props.onSelectWorkspace).toHaveBeenCalledTimes(1);
  });

  it("a clone this window started that fails neither moves the selection nor closes the dialog", async () => {
    const t = cloneHost();
    const props = await mountSidebar(t);
    await openGithub();
    fireEvent.click(screen.getByRole("button", { name: "Clone acme/web" }));
    await tick();

    await t.feedEvent("clone.changed", { clone: job({ state: "failed", error: { code: "failed", message: "x" } }) });
    expect(cloneRow("c1").getByRole("button", { name: "Retry" })).toBeTruthy();
    expect(props.onSelectWorkspace).not.toHaveBeenCalled();
    expect(githubDialog()).toBeTruthy();
  });

  it("never moves the selection for another client's clone", async () => {
    const t = cloneHost();
    const props = await mountSidebar(t);
    await openGithub();
    await t.feedEvent("clone.changed", { clone: job({ clone_id: "c9", state: "done", workspace_id: "w1", worktree_id: "w2" }) });
    expect(props.onSelectWorkspace).not.toHaveBeenCalled();
    expect(githubDialog()).toBeTruthy();
  });

  it("opens a repository already added as its click does, and closes the dialog", async () => {
    const t = cloneHost();
    t.handlers["workspace.list"] = () => [ws("w1"), ws("w2", { kind: "worktree", parent_workspace_id: "w1", branch: "zwiim/a" })];
    t.handlers["workspace.clone"] = () => ({ workspace_id: "w1" });
    const props = await mountSidebar(t);
    await openGithub();
    fireEvent.click(screen.getByRole("button", { name: "Clone acme/web" }));
    await tick();
    expect(props.onSelectWorkspace).toHaveBeenCalledWith("w2");
    expect(githubDialog()).toBeNull();
  });
});

const cloneRow = (id: string) => within(screen.getByTestId(`clone-${id}`));

describe("Clones in the workspace list", () => {
  it("shows a clone in flight with its progress; × cancels it", async () => {
    const t = cloneHost();
    t.clones = [job()];
    await mountSidebar(t);
    expect(cloneRow("c1").getByText("acme/web")).toBeTruthy();
    expect(cloneRow("c1").getByRole("progressbar").getAttribute("aria-valuenow")).toBe("42");
    // The sidebar stays uncolored.
    expect(screen.getByTestId("clone-c1").innerHTML).not.toContain("bg-brand");
    expect(screen.queryByText(/^No workspaces yet/)).toBeNull();

    fireEvent.click(cloneRow("c1").getByTitle("Cancel clone"));
    await tick();
    expect(t.calls.filter((c) => c.op === "workspace.clone_cancel").map((c) => c.params)).toEqual([{ clone_id: "c1" }]);

    await t.feedEvent("clone.changed", { clone: job({ state: "adding", phase: null, percent: null }) });
    expect((cloneRow("c1").getByTitle("Cancel clone") as HTMLButtonElement).disabled).toBe(true);
    await t.feedEvent("clone.changed", { clone: job({ state: "cancelled" }) });
    expect(screen.queryByTestId("clone-c1")).toBeNull();
    expect(screen.getByText(/^No workspaces yet/)).toBeTruthy();
  });

  it("shows a failed clone with its error; Dismiss puts it away", async () => {
    const t = cloneHost();
    t.clones = [job({ state: "failed", error: { code: "network", message: "GitHub could not be reached" } })];
    await mountSidebar(t);
    expect(cloneRow("c1").getByText("Couldn't reach GitHub.")).toBeTruthy();
    const before = t.calls.length;
    fireEvent.click(cloneRow("c1").getByRole("button", { name: "Dismiss" }));
    expect(screen.queryByTestId("clone-c1")).toBeNull();
    expect(t.calls.length).toBe(before);
  });

  it("a clone the signed-out GitHub CLI refused offers Sign in to GitHub on its row", async () => {
    const t = cloneHost();
    t.clones = [job({ state: "failed", error: { code: "gh_unauthenticated", message: "not logged in" } })];
    t.handlers["github.sign_in"] = () => ({
      state: "signing_in",
      code: "ABCD-1234",
      url: "https://github.com/login/device",
    });
    await mountSidebar(t);
    expect(cloneRow("c1").getByText("GitHub CLI isn't signed in on this Mac.")).toBeTruthy();
    fireEvent.click(cloneRow("c1").getByRole("button", { name: "Sign in to GitHub" }));
    await tick();
    expect(cloneRow("c1").getByLabelText("Sign-in code").textContent).toBe("ABCD-1234");
    expect(cloneRow("c1").getByRole("button", { name: "Retry" })).toBeTruthy();
  });

  it("Retry starts a new clone in place of the failed one", async () => {
    const t = cloneHost();
    t.handlers["workspace.clone"] = () => ({ clone_id: "c2" });
    t.clones = [job({ state: "failed", error: { code: "timeout", message: "stalled" } })];
    const props = await mountSidebar(t);
    fireEvent.click(cloneRow("c1").getByRole("button", { name: "Retry" }));
    await tick();
    expect(t.calls.filter((c) => c.op === "workspace.clone").map((c) => c.params)).toEqual([
      { owner: "acme", name: "web" },
    ]);
    expect(screen.queryByTestId("clone-c1")).toBeNull();

    await t.feedEvent("clone.changed", { clone: job({ clone_id: "c2", state: "queued", phase: null, percent: null }) });
    expect(screen.queryAllByTestId(/^clone-/).map((r) => r.getAttribute("data-testid"))).toEqual(["clone-c2"]);
    // The retried clone is this window's: done selects it.
    await t.feedEvent("clone.changed", { clone: job({ clone_id: "c2", state: "done", workspace_id: "w1", worktree_id: "w2" }) });
    expect(props.onSelectWorkspace).toHaveBeenCalledWith("w2");
  });

  it("a refused Retry shows why on the row", async () => {
    const t = cloneHost();
    t.handlers["workspace.clone"] = () => {
      throw new ZwiimClientError("clone_limit", "Too many clones are waiting", 409);
    };
    t.clones = [job({ state: "failed", error: { code: "failed", message: "x" } })];
    await mountSidebar(t);
    fireEvent.click(cloneRow("c1").getByRole("button", { name: "Retry" }));
    await tick();
    expect(cloneRow("c1").getByText("Too many clones are waiting")).toBeTruthy();
  });

  it("two clones from the dialog: the first done selects it and closes the dialog; the second goes on in the list", async () => {
    const t = cloneHost();
    const props = await mountSidebar(t);
    await openGithub();
    fireEvent.click(screen.getByRole("button", { name: "Clone acme/web" }));
    await tick();
    fireEvent.click(screen.getByRole("button", { name: "Clone acme/api" }));
    await tick();
    await t.feedEvent("clone.changed", { clone: job() });
    await t.feedEvent("clone.changed", { clone: job({ clone_id: "c2", name: "api", state: "queued", phase: null, percent: null }) });
    expect(screen.queryAllByTestId(/^clone-/).length).toBe(2);

    await t.feedEvent("clone.changed", { clone: job({ state: "done", workspace_id: "w1", worktree_id: "w2" }) });
    expect(props.onSelectWorkspace).toHaveBeenCalledWith("w2");
    expect(githubDialog()).toBeNull();
    expect(screen.queryByTestId("clone-c1")).toBeNull();
    expect(cloneRow("c2").getByText("acme/api")).toBeTruthy();

    await t.feedEvent("clone.changed", { clone: job({ clone_id: "c2", name: "api", state: "done", workspace_id: "w3", worktree_id: "w4" }) });
    expect(props.onSelectWorkspace).toHaveBeenLastCalledWith("w4");
  });
});
