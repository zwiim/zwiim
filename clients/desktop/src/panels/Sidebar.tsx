import { useEffect, useState } from "react";
import { ChevronDown, ChevronRight, Folder, FolderGit2, GitBranch, GitBranchPlus, Plus, Trash2 } from "lucide-react";
import {
  busyDetails,
  dirtyDetails,
  selectWorkspaceTree,
  type WorkspaceBusyDetails,
  type WorkspaceInfo,
  type WorkspaceNode,
} from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import { errorMessage, useController, useHost, useOnline } from "@/lib/host-context";
import { rememberSidebarFolds, sidebarFolds, type SidebarFolds } from "@/lib/prefs";
import { cn } from "@/lib/utils";
import { NewWorktree } from "./NewWorktree";

export interface SidebarProps {
  workspaceId: string | null;
  onSelectWorkspace(id: string): void;
  /** A new worktree started its first session. */
  onSessionStarted(workspaceId: string, sessionId: string): void;
  pickFolder(): Promise<string | null>;
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
const HOVER_ACTION = "opacity-0 group-hover:opacity-100 focus-visible:opacity-100";

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
  const [error, setError] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [folds, setFolds] = useState(sidebarFolds);

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
    const path = await props.pickFolder();
    if (!path) return;
    setAdding(true);
    try {
      const ws = await controller.addWorkspace(path);
      // The new workspace shows even if the list was folded away.
      if (folds.workspaces) fold({ ...folds, workspaces: false });
      props.onSelectWorkspace(ws.workspace_id);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setAdding(false);
    }
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-1.5">
      <div className="flex items-center pr-1">
        <button
          className="text-muted-foreground hover:text-foreground flex items-center gap-1 px-2 text-[11px] font-medium tracking-wider uppercase"
          aria-expanded={!folds.workspaces}
          onClick={() => fold({ ...folds, workspaces: !folds.workspaces })}
        >
          Workspaces
          <ChevronDown className={cn("size-3", folds.workspaces && "-rotate-90")} />
        </button>
        <Button
          className="text-muted-foreground ml-auto"
          size="icon-sm"
          variant="ghost"
          title="Add workspace"
          onClick={addWorkspace}
          disabled={adding || !online}
        >
          <Plus />
        </Button>
      </div>
      {error && <p className="text-destructive px-2 text-xs">{error}</p>}
      {!folds.workspaces && (
        <div className="-mx-1 flex min-h-0 flex-1 flex-col gap-px overflow-y-auto px-1" data-testid="workspace-tree">
          {!loaded && <p className="text-muted-foreground px-2 text-xs">Loading…</p>}
          {loaded && tree.length === 0 && (
            <p className="text-muted-foreground px-2 text-xs">No workspaces yet. Add a folder with +.</p>
          )}
          {tree.map((node) => (
            <WorkspaceRow
              key={node.workspace.workspace_id}
              node={node}
              {...props}
              folded={folds.repositories}
              onFold={foldRepository}
              onError={setError}
            />
          ))}
        </div>
      )}
    </div>
  );
}

type RowProps = SidebarProps & {
  node: WorkspaceNode;
  /** Repositories whose worktrees are folded away. */
  folded: readonly string[];
  onFold(id: string, folded: boolean): void;
  onError(message: string | null): void;
};

function WorkspaceRow(props: RowProps) {
  const online = useOnline();
  const { workspace: w, worktrees } = props.node;
  const isRoot = w.parent_workspace_id === null;
  const selected = props.workspaceId === w.workspace_id;
  const [creating, setCreating] = useState(false);
  const [removing, setRemoving] = useState(false);
  const folded = props.folded.includes(w.workspace_id);
  // Folded away, worktrees still show their working agents and selection on the repository.
  const hidden = folded ? worktrees : [];
  const working = props.node.working + hidden.reduce((n, c) => n + c.working, 0);
  const holdsSelected = hidden.some((n) => n.workspace.workspace_id === props.workspaceId);
  const base = baseLabel(w);

  return (
    <div data-testid={`workspace-${w.workspace_id}`}>
      <div
        className={cn(
          "group hover:bg-sidebar-accent flex items-center rounded-md pr-1",
          (selected || holdsSelected) && "bg-sidebar-accent",
        )}
      >
        {isRoot &&
          (worktrees.length > 0 ? (
            <button
              className="text-muted-foreground hover:text-foreground flex h-7 w-5 shrink-0 items-center justify-center"
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
          className={cn("flex min-w-0 flex-1 items-center gap-2 py-1.5 pr-1 text-left text-[13px]", !isRoot && "pl-2")}
          aria-current={selected || undefined}
          title={base ?? undefined}
          onClick={() => props.onSelectWorkspace(w.workspace_id)}
        >
          {isRoot && !w.is_git_repo ? (
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
          <span className="ml-auto flex min-w-0 items-center gap-1.5 text-[11px] font-normal">
            {isRoot && w.branch && <span className="text-muted-foreground min-w-0 truncate font-mono">{w.branch}</span>}
            {working > 0 && (
              <span
                className="flex size-3 shrink-0 items-center justify-center"
                title={`${plural(working, "agent", "agents")} working`}
              >
                <span className="bg-brand ring-brand/25 size-1.5 animate-pulse rounded-full ring-[3px]" />
                <span className="sr-only">{working} working</span>
              </span>
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
      {worktrees.length > 0 && !folded && (
        <div className="border-sidebar-border ml-[27px] flex flex-col gap-px border-l py-0.5 pl-1.5">
          {worktrees
            .filter((n) => !n.workspace.removed)
            .map((n) => (
              <WorkspaceRow key={n.workspace.workspace_id} {...props} node={n} />
            ))}
          <RemovedWorktrees {...props} nodes={worktrees.filter((n) => n.workspace.removed)} />
        </div>
      )}
    </div>
  );
}

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
        className="text-muted-foreground hover:text-foreground hover:bg-sidebar-accent flex items-center gap-2 rounded-md px-2 py-1 text-left text-xs"
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
    <div className="bg-card my-1 ml-2 flex flex-col gap-2 rounded-lg p-2.5 text-xs shadow-sm" role="alertdialog">
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
        <Button size="xs" variant="ghost" onClick={onClose}>
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
