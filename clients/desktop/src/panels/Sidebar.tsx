import { useEffect, useMemo, useRef, useState } from "react";
import {
  ChevronDown,
  ChevronRight,
  Folder,
  FolderGit2,
  FolderOpen,
  GitBranch,
  GitBranchPlus,
  GitMerge,
  History,
  Plus,
  Trash2,
  X,
} from "lucide-react";
import {
  busyDetails,
  conflictView,
  dirtyDetails,
  fullName,
  hasCapability,
  isActive,
  largeFolderDetails,
  largeFolderText,
  openTarget,
  progressLabel,
  sameRepo,
  selectWorkspaceTree,
  type CloneJob,
  type LargeFolderDetails,
  type WorkspaceBusyDetails,
  type WorkspaceInfo,
  type WorkspaceInspection,
  type WorkspaceNode,
} from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import { errorMessage, useController, useHost, useOnline } from "@/lib/host-context";
import { rememberSidebarFolds, sidebarFolds, type SidebarFolds } from "@/lib/prefs";
import { usePresence } from "@/lib/presence";
import { WorkingMark } from "@/components/WorkingMark";
import { cn } from "@/lib/utils";
import { moveMenuFocus } from "./ControlCenter";
import { GithubCloneDialog, GithubMark, ProgressBar, cloneErrorLine } from "./GithubCloneDialog";
import { GithubSignIn, useGithubSignInOffered } from "./Github";
import { NewWorktree } from "./NewWorktree";
import { PublishDialog, type PublishSubject } from "./PublishDialog";

export interface SidebarProps {
  workspaceId: string | null;
  onSelectWorkspace(id: string): void;
  /** A new worktree started its first session. */
  onSessionStarted(workspaceId: string, sessionId: string): void;
  pickFolder(): Promise<string | null>;
  /** Whether the Host is another computer (the publish dialog's "the host") or this one ("this Mac"). */
  remote: boolean;
  /** A read-only root session's Continue to a new worktree; each `nonce` is taken once, then `onContinued`. */
  continueRequest?: ContinueRequest | null;
  onContinued?(): void;
}

/** One Continue click: the root to continue, and a number no earlier click used. */
export interface ContinueRequest {
  root: string;
  nonce: number;
}

/** A worktree's label: its branch, or its folder name once the checkout is gone. */
export function worktreeLabel(w: WorkspaceInfo): string {
  return w.branch ?? w.name;
}

/** "base main@3f9e1a2" */
export function baseLabel(w: WorkspaceInfo): string | null {
  if (!w.base_commit) return null;
  return `base ${w.base_branch ?? "detached"}@${w.base_commit.slice(0, 7)}`;
}

/** Row actions stay out of the way until the row is hovered or focused. */
const HOVER_ACTION =
  "opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100 focus-visible:opacity-100";

/** The `+` menu's items, as the account menu's. */
const MENU_ITEM =
  "hover:bg-muted focus-visible:bg-muted flex w-full items-center gap-2.5 rounded-md px-2.5 py-1.5 text-left text-ui transition-colors disabled:opacity-50";

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** "2 agents running, 1 terminal open — stop them to remove." */
export function busyMessage(d: WorkspaceBusyDetails): string {
  const parts = [
    d.live_sessions > 0 && `${plural(d.live_sessions, "agent", "agents")} running`,
    d.terminals > 0 && `${plural(d.terminals, "terminal", "terminals")} open`,
  ].filter(Boolean);
  const one = d.live_sessions + d.terminals === 1;
  return `${parts.join(", ") || "Work is still running"} — stop ${one ? "it" : "them"} to remove.`;
}

export function Sidebar(props: SidebarProps) {
  const controller = useController();
  const online = useOnline();
  const tree = useHost(selectWorkspaceTree);
  const loaded = useHost((s) => s.workspaces !== null);
  // A Host from before publishing adds a picked folder as it is, as it always did.
  const publishable = useHost((s) => hasCapability(s.connection.compat, "workspace.publish"));
  const [error, setError] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  // A folder the Host refused as too large: a picked path, or a root added without GitHub.
  const [large, setLarge] = useState<{ subject: PublishSubject; details: LargeFolderDetails } | null>(null);
  // A picked folder or clicked root without an origin, asking before it becomes a GitHub repository.
  const [publishing, setPublishing] = useState<{ subject: PublishSubject; inspection: WorkspaceInspection } | null>(
    null,
  );
  // The root whose click is reading it or preparing its worktree.
  const [opening, setOpening] = useState<string | null>(null);
  // One inspect or dialog at a time, from + or a root's click.
  const busy = adding || opening !== null || publishing !== null;
  const [folds, setFolds] = useState(sidebarFolds);

  // A Host from before cloning has only folders to add: `+` picks one at once.
  const cloneable = useHost((s) => hasCapability(s.connection.compat, "workspace.clone"));
  const clones = useHost((s) => s.clones);
  const [menu, setMenu] = useState(false);
  const menuRoot = useRef<HTMLDivElement>(null);
  const menuList = useRef<HTMLDivElement>(null);
  const menuPresence = usePresence(menu);
  // Not part of `busy`: a clone's `{ workspace_id }` opens its root while the dialog is still open.
  const [github, setGithub] = useState(false);
  // Failed clones this window put away; the Host expires them on its own.
  const [dismissed, setDismissed] = useState<string[]>([]);
  // Clones this window started: only their `done` moves the selection.
  const [started, setStarted] = useState<string[]>([]);
  const jobs = useMemo(
    () => Object.values(clones).filter((j) => j.state !== "cancelled" && !dismissed.includes(j.clone_id)),
    [clones, dismissed],
  );
  // In flight or failed: a row in the workspace list, so closing the dialog loses nothing (spec §6).
  const pending = jobs.filter((j) => isActive(j) || j.state === "failed");

  useEffect(() => {
    if (!menu) return;
    menuList.current?.querySelector<HTMLElement>('[role="menuitem"]')?.focus();
    const onDown = (e: PointerEvent) => {
      if (!menuRoot.current?.contains(e.target as Node)) setMenu(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      setMenu(false);
      menuRoot.current?.querySelector<HTMLElement>('[aria-haspopup="menu"]')?.focus();
    };
    window.addEventListener("pointerdown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("pointerdown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [menu]);

  // Offline or busy, the menu's items could not run.
  useEffect(() => {
    if (!online || busy) setMenu(false);
  }, [online, busy]);

  // A clone this window started has ended: done selects its worktree and closes the dialog.
  useEffect(() => {
    const ended = started.map((id) => clones[id]).filter((j): j is CloneJob => j !== undefined && !isActive(j));
    if (ended.length === 0) return;
    setStarted((s) => s.filter((id) => !ended.some((j) => j.clone_id === id)));
    const done = ended.filter((j) => j.state === "done" && (j.worktree_id ?? j.workspace_id) !== null);
    const last = done[done.length - 1];
    if (!last) return;
    fold({
      workspaces: false,
      repositories: folds.repositories.filter((r) => !done.some((j) => j.workspace_id === r)),
    });
    props.onSelectWorkspace((last.worktree_id ?? last.workspace_id)!);
    setGithub(false);
  }, [clones, started]);

  function fold(next: SidebarFolds) {
    setFolds(next);
    rememberSidebarFolds(next);
  }

  function foldRepository(id: string, folded: boolean) {
    if (folds.repositories.includes(id) === folded) return;
    const repositories = folds.repositories.filter((r) => r !== id);
    fold({ ...folds, repositories: folded ? [...repositories, id] : repositories });
  }

  async function addWorkspace() {
    setError(null);
    setLarge(null);
    // Busy from the picker on, so a root's click can't inspect alongside this add.
    setAdding(true);
    try {
      const path = await props.pickFolder();
      if (!path) return;
      if (publishable) await addPicked(path);
      else await add(path, false);
    } finally {
      setAdding(false);
    }
  }

  /**
   * The folder as `workspace.inspect` reads it: one with an origin is added
   * at once; any other opens the publish dialog only when GitHub can be used,
   * and only its click adds (D11). Without GitHub it is added locally (D4).
   */
  async function addPicked(path: string) {
    try {
      const inspection = await controller.inspectWorkspace(path);
      if (inspection.git === "origin") await openFirstWorktree((await controller.addWorkspace(path)).workspace_id);
      else if (inspection.github?.available === false) await addLocal({ kind: "path", path }, false);
      else setPublishing({ subject: { kind: "path", path }, inspection });
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  /**
   * A repository just added opens in a new worktree, selected with its
   * composer, never the root. If the worktree fails the repository stays
   * added and the error shows. Never throws: the dialog's `onAdded` lands here.
   */
  async function openFirstWorktree(rootId: string) {
    try {
      // The new repository shows even if the list or the repository was folded away.
      fold({ workspaces: false, repositories: folds.repositories.filter((r) => r !== rootId) });
      const worktree = await controller.createWorktree(rootId);
      props.onSelectWorkspace(worktree.workspace_id);
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  /**
   * A root's click (§5.3): its `openTarget` worktree; without one, the root
   * as `workspace.inspect` reads it — a plain or unborn repository opens the
   * publish dialog, anything else gets a new worktree. Never selects the root,
   * except on a Host from before publishing, as it always did.
   */
  async function openRoot(rootId: string) {
    if (!publishable) return props.onSelectWorkspace(rootId);
    const target = openTarget(controller.store.getState(), rootId);
    if (target) return props.onSelectWorkspace(target.workspace_id);
    await newWorktree(rootId);
  }

  /**
   * A new worktree for `rootId` as its click makes one, even beside others: a
   * plain or unborn repository opens the publish dialog first, only when
   * GitHub can be used; without GitHub it is added locally first (D4).
   */
  async function newWorktree(rootId: string) {
    // Offline, an inspect could not be told apart from a lost one; a click that only selects still works.
    if (busy || !online) return;
    setError(null);
    setLarge(null);
    setOpening(rootId);
    try {
      const inspection = await controller.inspectRoot(rootId);
      if (inspection.git === "none" || (inspection.git === "no_origin" && inspection.first_commit)) {
        const subject: PublishSubject = { kind: "root", workspace_id: rootId };
        if (inspection.github?.available === false) await addLocal(subject, false);
        else setPublishing({ subject, inspection });
      } else await openFirstWorktree(rootId);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setOpening(null);
    }
  }

  /**
   * Start cloning `owner/name` (spec §6). A clone this window starts is
   * selected once done; a repository already added opens as its click does.
   * Once the Host accepts, an earlier failure of the same repository is put
   * away. Rejects with the Host's refusal, shown by the row that asked.
   */
  async function startClone(owner: string, name: string) {
    const repo = { owner, name };
    const failed = jobs.filter((j) => j.state === "failed" && sameRepo(j, repo)).map((j) => j.clone_id);
    const start = await controller.cloneWorkspace(owner, name);
    if (failed.length > 0) setDismissed((d) => [...d, ...failed]);
    if ("clone_id" in start) {
      // The new row shows even if the list was folded away.
      if (folds.workspaces) fold({ ...folds, workspaces: false });
      setStarted((s) => [...s, start.clone_id]);
      return;
    }
    setGithub(false);
    await openRoot(start.workspace_id);
  }

  // A root session's Continue (§5.3, D8): a new worktree, never its open target.
  // Each click runs once, though StrictMode runs the effect twice, as on a sidebar mounting with a request.
  const request = props.continueRequest ?? null;
  const taken = useRef(0);
  useEffect(() => {
    if (!request || request.nonce === taken.current) return;
    taken.current = request.nonce;
    props.onContinued?.();
    const root = controller.store.getState().workspaces?.find((w) => w.workspace_id === request.root);
    if (root) void newWorktree(root.workspace_id);
  }, [request?.nonce]);

  /**
   * Adds the folder as a local Git repository, with no GitHub repository
   * (its first commit included), then opens its first worktree: roots are
   * read-only (D7). A folder past the size limit shows the large card.
   */
  async function addLocal(subject: PublishSubject, allowLarge: boolean) {
    if (subject.kind === "path") setAdding(true);
    else setOpening(subject.workspace_id);
    try {
      const ws =
        subject.kind === "path"
          ? await controller.addWorkspace(subject.path, { allowLarge })
          : await controller.addRoot(subject.workspace_id, { allowLarge });
      setLarge(null);
      await openFirstWorktree(ws.workspace_id);
    } catch (e) {
      const details = largeFolderDetails(e);
      if (details) setLarge({ subject, details });
      else {
        setLarge(null);
        setError(errorMessage(e));
      }
    } finally {
      if (subject.kind === "path") setAdding(false);
      else setOpening(null);
    }
  }

  async function add(path: string, allowLarge: boolean) {
    setAdding(true);
    try {
      const ws = await controller.addWorkspace(path, { allowLarge });
      setLarge(null);
      // The new workspace shows even if the list was folded away.
      if (folds.workspaces) fold({ ...folds, workspaces: false });
      props.onSelectWorkspace(ws.workspace_id);
    } catch (e) {
      const details = largeFolderDetails(e);
      if (details) setLarge({ subject: { kind: "path", path }, details });
      else {
        setLarge(null);
        setError(errorMessage(e));
      }
    } finally {
      setAdding(false);
    }
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-1.5">
      <div className="flex items-center pr-1">
        <button
          className="text-muted-foreground hover:text-foreground text-2xs flex items-center gap-1 rounded-sm px-2 font-semibold tracking-wide uppercase transition-colors"
          aria-expanded={!folds.workspaces}
          onClick={() => fold({ ...folds, workspaces: !folds.workspaces })}
        >
          Workspaces
          <ChevronDown className={cn("size-3 transition-transform", folds.workspaces && "-rotate-90")} />
        </button>
        <div className="relative ml-auto" ref={menuRoot}>
          <Button
            className="text-muted-foreground"
            size="icon-sm"
            variant="ghost"
            title="Add workspace"
            aria-haspopup={cloneable ? "menu" : undefined}
            aria-expanded={cloneable ? menu : undefined}
            onClick={() => (cloneable ? setMenu((m) => !m) : void addWorkspace())}
            disabled={busy || !online}
          >
            <Plus />
          </Button>
          {menuPresence.mounted && (
            <div
              ref={menuList}
              className="bg-card text-card-foreground ring-border presence-pop shadow-pop absolute top-full right-0 z-40 mt-1 w-52 origin-top-right rounded-xl p-1.5 ring-1"
              role="menu"
              aria-label="Add workspace"
              data-state={menuPresence.state}
              aria-hidden={!menu || undefined}
              inert={!menu}
              onKeyDown={moveMenuFocus}
            >
              <button
                className={MENU_ITEM}
                role="menuitem"
                onClick={() => {
                  setMenu(false);
                  void addWorkspace();
                }}
              >
                <FolderOpen className="text-muted-foreground size-4" />
                Open project
              </button>
              <button
                className={MENU_ITEM}
                role="menuitem"
                onClick={() => {
                  setMenu(false);
                  setError(null);
                  setGithub(true);
                }}
              >
                <GithubMark className="text-muted-foreground" />
                Open GitHub project
              </button>
            </div>
          )}
        </div>
      </div>
      {error && <p className="text-destructive px-2 text-xs">{error}</p>}
      {publishing && (
        <PublishDialog
          // A new folder never inherits an earlier dialog's state.
          key={publishing.subject.kind === "path" ? publishing.subject.path : publishing.subject.workspace_id}
          subject={publishing.subject}
          inspection={publishing.inspection}
          remote={props.remote}
          onClose={() => setPublishing(null)}
          // Shows "Preparing workspace…" until the worktree step ends, either way;
          // then closes this dialog only, never one opened since.
          onAdded={(root) => {
            const shown = publishing;
            void openFirstWorktree(root.workspace_id).finally(() => setPublishing((p) => (p === shown ? null : p)));
          }}
        />
      )}
      {github && (
        <GithubCloneDialog remote={props.remote} jobs={jobs} onClose={() => setGithub(false)} onClone={startClone} />
      )}
      {large && (
        <div
          className="bg-card animate-pop-in mx-1 flex flex-col gap-2 rounded-lg p-2.5 text-xs shadow-sm"
          role="alertdialog"
          aria-label="Large folder"
        >
          <p className="font-medium">Large folder</p>
          <p>
            Zwiim will commit <strong>{largeFolderText(large.details)}</strong> as "Initial Commit". Add it anyway?
          </p>
          <div className="flex justify-end gap-1.5">
            {/* The Host finishes an add it has started, so Cancel cannot stop it. */}
            <Button size="xs" variant="ghost" disabled={busy} onClick={() => setLarge(null)}>
              Cancel
            </Button>
            <Button
              size="xs"
              variant="brand"
              disabled={busy || !online}
              onClick={() => {
                const { subject } = large;
                if (publishable) void addLocal(subject, true);
                else if (subject.kind === "path") void add(subject.path, true);
              }}
            >
              Add anyway
            </Button>
          </div>
        </div>
      )}
      {!folds.workspaces && (
        <div
          className="animate-fade-in -mx-1 flex min-h-0 flex-1 flex-col gap-px overflow-y-auto px-1"
          data-testid="workspace-tree"
        >
          {!loaded && <p className="text-muted-foreground px-2 text-xs">Loading…</p>}
          {pending.map((job) => (
            <CloneRow
              key={job.clone_id}
              job={job}
              remote={props.remote}
              onRetry={() => startClone(job.owner, job.name)}
              onDismiss={() => setDismissed((d) => [...d, job.clone_id])}
            />
          ))}
          {loaded && tree.length === 0 && pending.length === 0 && (
            <p className="text-muted-foreground px-2 text-xs">No workspaces yet. Add a folder with +.</p>
          )}
          {tree.map((node) => (
            <WorkspaceRow
              key={node.workspace.workspace_id}
              node={node}
              {...props}
              publishable={publishable}
              folded={folds.repositories}
              onFold={foldRepository}
              onError={setError}
              opening={opening}
              onOpenRoot={openRoot}
            />
          ))}
        </div>
      )}
    </div>
  );
}

/**
 * A clone in the workspace list: muted `owner/name` with an uncolored
 * progress bar and a cancel `×`, or its failure with Retry and Dismiss.
 */
function CloneRow(props: { job: CloneJob; remote: boolean; onRetry(): Promise<void>; onDismiss(): void }) {
  const { job } = props;
  const controller = useController();
  const online = useOnline();
  const signInOffered = useGithubSignInOffered();
  const [error, setError] = useState<string | null>(null);
  const [retrying, setRetrying] = useState(false);
  const failed = job.state === "failed";
  const host = props.remote ? "the host" : "this Mac";

  async function cancel() {
    setError(null);
    try {
      await controller.cancelClone(job.clone_id);
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  async function retry() {
    setError(null);
    setRetrying(true);
    try {
      await props.onRetry();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setRetrying(false);
    }
  }

  return (
    <div className="flex flex-col gap-1 py-1 pr-1" data-testid={`clone-${job.clone_id}`}>
      <div className="flex items-center gap-2">
        <span className="w-5 shrink-0" />
        <FolderGit2 className="text-muted-foreground size-3.5 shrink-0" />
        <span className="text-muted-foreground min-w-0 flex-1 truncate font-mono text-ui">{fullName(job)}</span>
        {!failed && (
          <Button
            className="text-muted-foreground"
            size="icon-sm"
            variant="ghost"
            title="Cancel clone"
            disabled={job.state === "adding" || !online}
            onClick={() => void cancel()}
          >
            <X />
          </Button>
        )}
      </div>
      {!failed && (
        <div className="pr-2 pl-7">
          <ProgressBar percent={job.percent} fill="bg-muted-foreground/60" label={progressLabel(job)} />
        </div>
      )}
      {failed && (
        <div className="flex flex-col gap-1 pl-7">
          <p className="text-destructive text-xs">{cloneErrorLine(job, host, signInOffered)}</p>
          {job.error?.code === "gh_unauthenticated" && signInOffered && <GithubSignIn host={host} />}
          <div className="flex gap-1">
            <Button size="xs" variant="ghost" disabled={retrying || !online} onClick={() => void retry()}>
              Retry
            </Button>
            <Button size="xs" variant="ghost" onClick={props.onDismiss}>
              Dismiss
            </Button>
          </div>
        </div>
      )}
      {error && <p className="text-destructive pl-7 text-xs">{error}</p>}
    </div>
  );
}

type RowProps = SidebarProps & {
  node: WorkspaceNode;
  /** Repositories whose worktrees are folded away. */
  folded: readonly string[];
  onFold(id: string, folded: boolean): void;
  onError(message: string | null): void;
  /** The root whose click is reading it or preparing its worktree. */
  opening: string | null;
  onOpenRoot(rootId: string): void;
  /** A current Host: a root is selected only read-only, through its earlier-sessions row. */
  publishable: boolean;
};

function WorkspaceRow(props: RowProps) {
  const online = useOnline();
  const { workspace: w, worktrees } = props.node;
  const isRoot = w.parent_workspace_id === null;
  // On a current Host a root's own sessions are read-only history under one row (§5.3, D8).
  const earlier = isRoot && props.publishable ? (props.node.sessions?.length ?? 0) : 0;
  const earlierSelected = earlier > 0 && props.workspaceId === w.workspace_id;
  const selected = props.workspaceId === w.workspace_id && !earlierSelected;
  const nested = worktrees.length > 0 || earlier > 0;
  const [creating, setCreating] = useState(false);
  const [removing, setRemoving] = useState(false);
  const folded = props.folded.includes(w.workspace_id);
  // Folded away, worktrees still show their working agents and selection on the repository.
  const hidden = folded ? worktrees : [];
  const working = props.node.working + hidden.reduce((n, c) => n + c.working, 0);
  const holdsSelected =
    hidden.some((n) => n.workspace.workspace_id === props.workspaceId) || (folded && earlierSelected);
  const base = baseLabel(w);
  const preparing = props.opening === w.workspace_id;
  const conflicts = useHost((s) => conflictView(s, w.workspace_id)?.line ?? null);

  return (
    <div data-testid={`workspace-${w.workspace_id}`}>
      <div
        className={cn(
          "group hover:bg-sidebar-accent flex items-center rounded-md pr-1 transition-colors",
          (selected || holdsSelected) && "bg-sidebar-accent",
        )}
      >
        {isRoot &&
          (nested ? (
            <button
              className="text-muted-foreground hover:text-foreground flex h-7 w-5 shrink-0 items-center justify-center rounded-sm transition-colors"
              title={folded ? "Show worktrees" : "Hide worktrees"}
              aria-expanded={!folded}
              onClick={() => props.onFold(w.workspace_id, !folded)}
            >
              {folded ? <ChevronRight className="size-3.5" /> : <ChevronDown className="size-3.5" />}
            </button>
          ) : (
            <span className="w-5 shrink-0" />
          ))}
        <button
          className={cn("text-ui flex min-w-0 flex-1 items-center gap-2 py-1.5 pr-1 text-left", !isRoot && "pl-2")}
          aria-current={selected || undefined}
          title={base ?? undefined}
          onClick={() => (isRoot ? props.onOpenRoot(w.workspace_id) : props.onSelectWorkspace(w.workspace_id))}
        >
          {working > 0 ? (
            // A row with an agent at work trades its icon for the working mark.
            <WorkingMark className={isRoot ? "size-4" : undefined} />
          ) : isRoot && !w.is_git_repo ? (
            <Folder className="text-muted-foreground size-4 shrink-0" aria-label="Folder, not a Git repository" />
          ) : isRoot ? (
            <FolderGit2 className="text-muted-foreground size-4 shrink-0" />
          ) : (
            <GitBranch className="text-muted-foreground size-3.5 shrink-0" />
          )}
          <span
            className={cn(
              "truncate",
              // The name keeps its room; the branch beside it gives way first.
              isRoot && w.branch ? "max-w-[70%] shrink-0" : "min-w-0",
              isRoot ? "font-medium" : "font-mono text-xs",
              w.removed && "text-muted-foreground line-through",
            )}
          >
            {isRoot ? (w.alias ?? w.name) : worktreeLabel(w)}
          </span>
          <span className="ml-auto flex min-w-0 items-center gap-1.5 text-2xs font-normal">
            {preparing ? (
              <span className="text-muted-foreground min-w-0 truncate">Preparing workspace…</span>
            ) : (
              isRoot && w.branch && <span className="text-muted-foreground min-w-0 truncate font-mono">{w.branch}</span>
            )}
            {conflicts && (
              <GitMerge className="text-brand size-3 shrink-0" role="img" aria-label={conflicts}>
                <title>{conflicts}</title>
              </GitMerge>
            )}
            {working > 0 && (
              <span className="sr-only">{working} working</span>
            )}
          </span>
        </button>
        {isRoot && w.is_git_repo && (
          <Button
            className={HOVER_ACTION}
            size="icon-sm"
            variant="ghost"
            title="New worktree"
            onClick={() => {
              props.onError(null);
              setCreating(true);
            }}
            disabled={!online}
          >
            <GitBranchPlus />
          </Button>
        )}
        {!isRoot && !w.removed && (
          <Button
            className={HOVER_ACTION}
            size="icon-sm"
            variant="ghost"
            title="Remove worktree"
            disabled={!online}
            onClick={() => setRemoving(true)}
          >
            <Trash2 />
          </Button>
        )}
      </div>
      {creating && (
        <NewWorktree
          rootId={w.workspace_id}
          onClose={() => setCreating(false)}
          onCreated={(id) => {
            props.onFold(w.workspace_id, false);
            props.onSelectWorkspace(id);
          }}
          onStarted={props.onSessionStarted}
        />
      )}
      {removing && <RemoveWorktree workspace={w} onClose={() => setRemoving(false)} />}
      {nested && !folded && (
        <div className="border-sidebar-border animate-fade-in ml-[27px] flex flex-col gap-px border-l py-0.5 pl-1.5">
          {worktrees
            .filter((n) => !n.workspace.removed)
            .map((n) => (
              <WorkspaceRow key={n.workspace.workspace_id} {...props} node={n} />
            ))}
          <RemovedWorktrees {...props} nodes={worktrees.filter((n) => n.workspace.removed)} />
          {earlier > 0 && (
            <button
              className={cn(FOLDED_ROW, earlierSelected && "bg-sidebar-accent text-foreground")}
              aria-current={earlierSelected || undefined}
              onClick={() => props.onSelectWorkspace(w.workspace_id)}
            >
              <History className="size-3.5" />
              {plural(earlier, "earlier session", "earlier sessions")}
            </button>
          )}
        </div>
      )}
    </div>
  );
}

/** A muted row standing for history folded beneath a repository. */
const FOLDED_ROW =
  "text-muted-foreground hover:text-foreground hover:bg-sidebar-accent flex items-center gap-2 rounded-md px-2 py-1 text-left text-xs transition-colors";

/** Removed worktrees keep their sessions' history, folded under one row. */
function RemovedWorktrees({ nodes, ...row }: RowProps & { nodes: WorkspaceNode[] }) {
  const holdsSelected = nodes.some((n) => n.workspace.workspace_id === row.workspaceId);
  const [open, setOpen] = useState(holdsSelected);
  useEffect(() => {
    if (holdsSelected) setOpen(true);
  }, [holdsSelected]);
  if (nodes.length === 0) return null;
  return (
    <>
      <button
        className={FOLDED_ROW}
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        {open ? <ChevronDown className="size-3.5" /> : <ChevronRight className="size-3.5" />}
        {nodes.length} removed
      </button>
      {open && nodes.map((n) => <WorkspaceRow key={n.workspace.workspace_id} {...row} node={n} />)}
    </>
  );
}

type RemovePhase = { kind: "confirm" } | { kind: "dirty"; changes: number } | { kind: "busy"; message: string };

/**
 * Removal as the host decides it: a plain request first; the host answers
 * `workspace_busy` (nothing to confirm — stop the work) or `workspace_dirty`
 * (confirm the loss, then force). Force never bypasses the busy guard.
 */
function RemoveWorktree({ workspace, onClose }: { workspace: WorkspaceInfo; onClose(): void }) {
  const controller = useController();
  const online = useOnline();
  const [phase, setPhase] = useState<RemovePhase>({ kind: "confirm" });
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const branch = worktreeLabel(workspace);

  async function remove(force: boolean) {
    setPending(true);
    setError(null);
    try {
      await controller.removeWorktree(workspace.workspace_id, { force });
      onClose();
    } catch (e) {
      const busy = busyDetails(e);
      const dirty = dirtyDetails(e);
      if (busy) setPhase({ kind: "busy", message: busyMessage(busy) });
      else if (dirty) setPhase({ kind: "dirty", changes: dirty.changes });
      else setError(errorMessage(e));
    } finally {
      setPending(false);
    }
  }

  return (
    <div className="bg-card animate-pop-in my-1 ml-2 flex flex-col gap-2 rounded-lg p-2.5 text-xs shadow-sm" role="alertdialog">
      {phase.kind === "confirm" && (
        <p>
          Remove worktree <span className="font-mono">{branch}</span>? Branch{" "}
          <span className="font-mono">{branch}</span> will be kept.
        </p>
      )}
      {phase.kind === "dirty" && (
        <p>
          {plural(phase.changes, "uncommitted change", "uncommitted changes")} will be lost. Branch{" "}
          <span className="font-mono">{branch}</span> will be kept.
        </p>
      )}
      {phase.kind === "busy" && <p className="text-destructive">{phase.message}</p>}
      {error && <p className="text-destructive">{error}</p>}
      <div className="flex justify-end gap-1.5">
        {/* Focus lands on the safe choice (DESIGN.md: never on the destructive one). */}
        <Button size="xs" variant="ghost" autoFocus onClick={onClose}>
          Cancel
        </Button>
        {phase.kind === "dirty" ? (
          <Button size="xs" variant="destructive" disabled={pending || !online} onClick={() => remove(true)}>
            Discard and remove
          </Button>
        ) : (
          <Button size="xs" variant="destructive" disabled={pending || !online} onClick={() => remove(false)}>
            {phase.kind === "busy" ? "Try again" : "Remove"}
          </Button>
        )}
      </div>
    </div>
  );
}
