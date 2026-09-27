import { useState } from "react";
import { Folder, FolderGit2, GitBranch, GitBranchPlus, Plus, Trash2 } from "lucide-react";
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

  async function addWorkspace() {
    setError(null);
    const path = await props.pickFolder();
    if (!path) return;
    setAdding(true);
    try {
      const ws = await controller.addWorkspace(path);
      props.onSelectWorkspace(ws.workspace_id);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setAdding(false);
    }
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-1.5">
      <span className="text-muted-foreground px-2 text-[11px] font-medium tracking-wider uppercase">Workspaces</span>
      {error && <p className="text-destructive px-2 text-xs">{error}</p>}
      <div className="-mx-1 flex min-h-0 flex-1 flex-col gap-px overflow-y-auto px-1" data-testid="workspace-tree">
        {!loaded && <p className="text-muted-foreground px-2 text-xs">Loading…</p>}
        {loaded && tree.length === 0 && (
          <p className="text-muted-foreground px-2 text-xs">No workspaces yet. Add a folder to begin.</p>
        )}
        {tree.map((node) => (
          <WorkspaceRow key={node.workspace.workspace_id} node={node} {...props} onError={setError} />
        ))}
        <button
          className="text-muted-foreground hover:text-foreground hover:bg-sidebar-accent mt-2 flex items-center gap-2 rounded-md px-2 py-1.5 text-left text-[13px] disabled:opacity-50"
          title="Add workspace"
          onClick={addWorkspace}
          disabled={adding || !online}
        >
          <Plus className="size-4" /> Add workspace
        </button>
      </div>
    </div>
  );
}

function WorkspaceRow(props: SidebarProps & { node: WorkspaceNode; onError(message: string | null): void }) {
  const online = useOnline();
  const { workspace: w, working, worktrees } = props.node;
  const isRoot = w.parent_workspace_id === null;
  const selected = props.workspaceId === w.workspace_id;
  const [creating, setCreating] = useState(false);
  const [removing, setRemoving] = useState(false);

  return (
    <div data-testid={`workspace-${w.workspace_id}`}>
      <div
        className={cn(
          "group hover:bg-sidebar-accent flex items-center gap-0.5 rounded-md pr-1",
          selected && "bg-sidebar-accent",
        )}
      >
        <button
          className="flex min-w-0 flex-1 items-center gap-2 px-2 py-1.5 text-left text-[13px]"
          aria-current={selected || undefined}
          onClick={() => props.onSelectWorkspace(w.workspace_id)}
        >
          {isRoot && !w.is_git_repo ? (
            <Folder className="text-muted-foreground size-4 shrink-0" aria-label="Folder, not a Git repository" />
          ) : isRoot ? (
            <FolderGit2 className="text-muted-foreground size-4 shrink-0" />
          ) : (
            <GitBranch className="text-muted-foreground size-3.5 shrink-0" />
          )}
          <span className="flex min-w-0 flex-col">
            <span
              className={cn("truncate", isRoot && "font-medium", w.removed && "text-muted-foreground line-through")}
            >
              {isRoot ? (w.alias ?? w.name) : worktreeLabel(w)}
            </span>
            {!isRoot && baseLabel(w) && (
              <span className="text-muted-foreground truncate font-mono text-[10px] font-normal">{baseLabel(w)}</span>
            )}
          </span>
          <span className="ml-auto flex shrink-0 items-center gap-1.5 text-[11px] font-normal">
            {working > 0 && (
              <span className="flex items-center gap-1 text-amber-600 dark:text-amber-400" title="Agents working">
                <span className="size-1.5 animate-pulse rounded-full bg-amber-500" />
                {working} working
              </span>
            )}
            {isRoot && w.branch && <span className="text-muted-foreground font-mono">{w.branch}</span>}
            {w.removed && <span className="text-muted-foreground">removed</span>}
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
          onCreated={props.onSelectWorkspace}
          onStarted={props.onSessionStarted}
        />
      )}
      {removing && <RemoveWorktree workspace={w} onClose={() => setRemoving(false)} />}
      {worktrees.length > 0 && (
        <div className="border-sidebar-border ml-[15px] flex flex-col gap-px border-l py-0.5 pl-1.5">
          {worktrees.map((n) => (
            <WorkspaceRow key={n.workspace.workspace_id} {...props} node={n} />
          ))}
        </div>
      )}
    </div>
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
