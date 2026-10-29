import { useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { createPortal } from "react-dom";
import {
  Archive,
  ChevronDown,
  ChevronRight,
  Ellipsis,
  Folder,
  FolderGit2,
  FolderMinus,
  FolderOpen,
  GitBranch,
  GitBranchPlus,
  GitMerge,
  History,
  Plus,
  Settings2,
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
  type ScriptRun,
  type WorktreeRemoved,
} from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import { errorMessage, useController, useHost, useOnline } from "@/lib/host-context";
import { rememberSidebarFolds, sidebarFolds, type SidebarFolds } from "@/lib/prefs";
import { usePresence } from "@/lib/presence";
import { WorkingMark } from "@/components/WorkingMark";
import { cn } from "@/lib/utils";
import { useOpenSettings } from "@/settings/open";
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

/** "2 agents running, 1 terminal open — stop them to archive." */
export function busyMessage(d: WorkspaceBusyDetails, verb = "archive"): string {
  const parts = [
    d.live_sessions > 0 && `${plural(d.live_sessions, "agent", "agents")} running`,
    d.terminals > 0 && `${plural(d.terminals, "terminal", "terminals")} open`,
  ].filter(Boolean);
  const one = d.live_sessions + d.terminals === 1;
  return `${parts.join(", ") || "Work is still running"} — stop ${one ? "it" : "them"} to ${verb}.`;
}

export function Sidebar(props: SidebarProps) {
  const controller = useController();
  const online = useOnline();
  const tree = useHost(selectWorkspaceTree);
  const loaded = useHost((s) => s.workspaces !== null);
  // A Host from before publishing adds a picked folder as it is, as it always did.
  const publishable = useHost((s) => hasCapability(s.connection.compat, "workspace.publish"));
  const [error, setError] = useState<string | null>(null);
  // What became of a removed worktree's branch, until dismissed.
  const [note, setNote] = useState<string | null>(null);
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
      {note && (
        <p className="text-muted-foreground flex items-start gap-1 px-2 text-xs" role="status">
          <span className="min-w-0 flex-1">{note}</span>
          <button aria-label="Dismiss" className="hover:text-foreground shrink-0" onClick={() => setNote(null)}>
            <X className="size-3" />
          </button>
        </p>
      )}
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
              onNote={setNote}
              opening={opening}
              onOpenRoot={openRoot}
              onNewWorktree={(id) => void newWorktree(id)}
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
  /** What became of a removed worktree's branch. */
  onNote(message: string | null): void;
  /** The root whose click is reading it or preparing its worktree. */
  opening: string | null;
  onOpenRoot(rootId: string): void;
  /** A new worktree from the root's current branch, as a click on a root without one makes. */
  onNewWorktree(rootId: string): void;
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
  const [menu, setMenu] = useState(false);
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
        {isRoot && (
          <RepositoryMenu
            workspace={w}
            open={menu}
            onOpen={setMenu}
            onNewWorktree={() => {
              props.onError(null);
              props.onNewWorktree(w.workspace_id);
            }}
            onNewWorktreeFrom={() => {
              props.onError(null);
              setCreating(true);
            }}
            onRemove={() => setRemoving(true)}
          />
        )}
        {!isRoot && !w.removed && (
          <Button
            className={HOVER_ACTION}
            size="icon-sm"
            variant="ghost"
            title="Archive worktree"
            disabled={!online}
            onClick={() => setRemoving(true)}
          >
            <Archive />
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
      {removing && isRoot && (
        <RemoveWorkspace
          workspace={w}
          worktrees={worktrees.filter((n) => !n.workspace.removed).length}
          onClose={() => setRemoving(false)}
        />
      )}
      {removing && !isRoot && (
        <RemoveWorktree
          workspace={w}
          onClose={() => setRemoving(false)}
          onRemoved={(r) => props.onNote(removalNote(r))}
        />
      )}
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

/** Archived worktrees keep their sessions' history, folded under one row. */
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
        {nodes.length} archived
      </button>
      {open && nodes.map((n) => <WorkspaceRow key={n.workspace.workspace_id} {...row} node={n} />)}
    </>
  );
}

type RemovePhase = { kind: "confirm" } | { kind: "dirty"; changes: number } | { kind: "busy"; message: string };

/** What to say of a worktree just archived: how its archive script failed, and its branch; null when nothing. */
export function removalNote(r: WorktreeRemoved): string | null {
  const notes = [archiveFailure(r.archive_script), branchOutcome(r)].filter((n): n is string => n !== null);
  return notes.length > 0 ? notes.join(" ") : null;
}

function archiveFailure(run: ScriptRun | null | undefined): string | null {
  if (!run || run.exit_code === 0) return null;
  if (run.timed_out) return "The archive script was stopped after 5 minutes.";
  if (run.exit_code !== null) return `The archive script failed with exit code ${run.exit_code}.`;
  return "The archive script did not finish.";
}

function branchOutcome(r: WorktreeRemoved): string | null {
  if (r.branch_deleted && r.branch) return `Deleted branch ${r.branch}.`;
  if (r.branch_kept_reason) return `Kept the branch: ${r.branch_kept_reason}.`;
  return null;
}

/**
 * Removal as the host decides it: a plain request first; the host answers
 * `workspace_busy` (nothing to confirm — stop the work) or `workspace_dirty`
 * (confirm the loss, then force). Force never bypasses the busy guard. The
 * host's Git settings decide whether the branch goes too; the host deletes it
 * only when no work is lost, and says why it kept it otherwise.
 */
function RemoveWorktree({
  workspace,
  onClose,
  onRemoved,
}: {
  workspace: WorkspaceInfo;
  onClose(): void;
  onRemoved(removed: WorktreeRemoved): void;
}) {
  const controller = useController();
  const online = useOnline();
  const gitSettings = useHost((s) => hasCapability(s.connection.compat, "settings.git"));
  const [phase, setPhase] = useState<RemovePhase>({ kind: "confirm" });
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Whether the host deletes the branch too; unknown reads as kept.
  const [deletes, setDeletes] = useState(false);
  const branch = worktreeLabel(workspace);

  useEffect(() => {
    if (!gitSettings) return;
    let live = true;
    controller.client.request("settings.get", {}).then(
      (s) => live && setDeletes(s.values["git.delete_branch_on_remove"] === true),
      () => {},
    );
    return () => {
      live = false;
    };
  }, [controller, gitSettings]);

  const fate = deletes ? (
    <>
      Branch <span className="font-mono">{branch}</span> is deleted if its pull request merged or every commit is
      pushed, and kept otherwise.
    </>
  ) : (
    <>
      Branch <span className="font-mono">{branch}</span> will be kept.
    </>
  );

  async function remove(force: boolean) {
    setPending(true);
    setError(null);
    try {
      const removed = await controller.removeWorktree(workspace.workspace_id, { force });
      onRemoved(removed);
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
          Archive worktree <span className="font-mono">{branch}</span>? {fate}
        </p>
      )}
      {phase.kind === "dirty" && (
        <p>
          {plural(phase.changes, "uncommitted change", "uncommitted changes")} will be lost. {fate}
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
            Discard and archive
          </Button>
        ) : (
          <Button size="xs" variant="destructive" disabled={pending || !online} onClick={() => remove(false)}>
            {phase.kind === "busy" ? "Try again" : "Archive"}
          </Button>
        )}
      </div>
    </div>
  );
}

/**
 * A repository's `⋯`: a new worktree from its current branch or from one
 * picked, its settings, and Remove. Drawn over the page so the scrolling
 * list never clips it.
 */
function RepositoryMenu(props: {
  workspace: WorkspaceInfo;
  open: boolean;
  onOpen(open: boolean): void;
  onNewWorktree(): void;
  onNewWorktreeFrom(): void;
  onRemove(): void;
}) {
  const { workspace: w, open, onOpen } = props;
  const online = useOnline();
  const openSettings = useOpenSettings();
  const configurable = useHost((s) => hasCapability(s.connection.compat, "repository.settings"));
  const removable = useHost((s) => hasCapability(s.connection.compat, "workspace.remove"));
  const root = useRef<HTMLDivElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const [anchor, setAnchor] = useState<DOMRect | null>(null);
  const presence = usePresence(open);

  useEffect(() => {
    if (!open) return;
    list.current?.querySelector<HTMLElement>('[role="menuitem"]')?.focus();
    const onDown = (e: PointerEvent) => {
      const t = e.target as Node;
      if (!root.current?.contains(t) && !list.current?.contains(t)) onOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      onOpen(false);
      root.current?.querySelector<HTMLElement>('[aria-haspopup="menu"]')?.focus();
    };
    window.addEventListener("pointerdown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("pointerdown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [open, onOpen]);

  // Offline, none of its items could run.
  useEffect(() => {
    if (!online) onOpen(false);
  }, [online, onOpen]);

  function pick(run: () => void) {
    onOpen(false);
    run();
  }

  // Low in the list, the menu opens upward.
  const up = anchor ? anchor.bottom > window.innerHeight * 0.6 : false;
  const style: CSSProperties | undefined = anchor
    ? {
        left: anchor.right,
        ...(up ? { bottom: window.innerHeight - anchor.top + 4 } : { top: anchor.bottom + 4 }),
        transform: "translateX(-100%)",
        transformOrigin: up ? "bottom right" : "top right",
      }
    : undefined;
  const name = w.alias ?? w.name;
  const configure = openSettings && configurable;
  if (!w.is_git_repo && !configure && !removable) return null;

  return (
    <div ref={root} className={cn("flex", !open && HOVER_ACTION)}>
      <Button
        size="icon-sm"
        variant="ghost"
        title="Repository actions"
        aria-label={`${name} actions`}
        aria-haspopup="menu"
        aria-expanded={open}
        disabled={!online}
        onClick={() => {
          if (!open) setAnchor(root.current!.getBoundingClientRect());
          onOpen(!open);
        }}
      >
        <Ellipsis />
      </Button>
      {presence.mounted &&
        createPortal(
          <div
            ref={list}
            className="bg-card text-card-foreground ring-border presence-pop shadow-pop fixed z-50 w-52 rounded-xl p-1.5 ring-1"
            style={style}
            role="menu"
            aria-label={`${name} actions`}
            data-state={presence.state}
            aria-hidden={!open || undefined}
            inert={!open}
            onKeyDown={moveMenuFocus}
          >
            {w.is_git_repo && (
              <>
                <button className={MENU_ITEM} role="menuitem" onClick={() => pick(props.onNewWorktree)}>
                  <GitBranchPlus className="text-muted-foreground size-4" />
                  New worktree
                </button>
                <button className={MENU_ITEM} role="menuitem" onClick={() => pick(props.onNewWorktreeFrom)}>
                  <GitBranch className="text-muted-foreground size-4" />
                  New worktree from…
                </button>
              </>
            )}
            {configure && (
              <button
                className={MENU_ITEM}
                role="menuitem"
                onClick={() => pick(() => openSettings({ tab: "repository", workspaceId: w.workspace_id }))}
              >
                <Settings2 className="text-muted-foreground size-4" />
                Configuration
              </button>
            )}
            {removable && (
              <>
                <div className="bg-border my-1 h-px" role="separator" />
                <button className={cn(MENU_ITEM, "text-destructive")} role="menuitem" onClick={() => pick(props.onRemove)}>
                  <FolderMinus className="size-4" />
                  Remove
                </button>
              </>
            )}
          </div>,
          document.body,
        )}
    </div>
  );
}

/**
 * Removing a repository from Zwiim, asked as a worktree's archive is: a plain
 * request first, then the Host's `workspace_busy` (stop the work) or
 * `workspace_dirty` (confirm the loss, then force). Its worktrees are archived
 * and the row goes; the folder itself is never touched, and adding it again
 * brings the repository back with its sessions.
 */
function RemoveWorkspace({
  workspace,
  worktrees,
  onClose,
}: {
  workspace: WorkspaceInfo;
  /** Worktrees not yet archived. */
  worktrees: number;
  onClose(): void;
}) {
  const controller = useController();
  const online = useOnline();
  const [phase, setPhase] = useState<RemovePhase>({ kind: "confirm" });
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const name = workspace.alias ?? workspace.name;
  const folder = workspace.display_path ? (
    <span className="font-mono">{workspace.display_path}</span>
  ) : (
    "its folder"
  );

  async function remove(force: boolean) {
    setPending(true);
    setError(null);
    try {
      await controller.removeWorkspace(workspace.workspace_id, { force });
      onClose();
    } catch (e) {
      const busy = busyDetails(e);
      const dirty = dirtyDetails(e);
      if (busy) setPhase({ kind: "busy", message: busyMessage(busy, "remove") });
      else if (dirty) setPhase({ kind: "dirty", changes: dirty.changes });
      else setError(errorMessage(e));
    } finally {
      setPending(false);
    }
  }

  return (
    <div
      className="bg-card animate-pop-in my-1 ml-2 flex flex-col gap-2 rounded-lg p-2.5 text-xs shadow-sm"
      role="alertdialog"
      aria-label={`Remove ${name}`}
    >
      {phase.kind === "confirm" && (
        <p>
          Remove <span className="font-medium">{name}</span>
          {worktrees > 0 && <> and archive its {worktrees === 1 ? "worktree" : `${worktrees} worktrees`}</>}? Files in {folder}{" "}
          are not changed. Add the folder again to bring it back.
        </p>
      )}
      {phase.kind === "dirty" && (
        <p>
          {plural(phase.changes, "uncommitted change", "uncommitted changes")} in its worktrees will be lost.
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
