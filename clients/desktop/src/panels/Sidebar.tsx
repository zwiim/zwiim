import { useState } from "react";
import { FolderGit2, FolderPlus, GitBranch, GitBranchPlus, MessageSquare, Plus, Trash2 } from "lucide-react";
import {
  busyDetails,
  dirtyDetails,
  isSessionWorking,
  selectWorkspaceTree,
  type SessionSummary,
  type WorkspaceBusyDetails,
  type WorkspaceInfo,
  type WorkspaceNode,
} from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import { errorMessage, useController, useHost, useOnline } from "@/lib/host-context";
import { cn } from "@/lib/utils";

export interface SidebarProps {
  workspaceId: string | null;
  sessionId: string | null;
  onSelectWorkspace(id: string): void;
  onSelectSession(workspaceId: string, sessionId: string | null): void;
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
    <div className="flex min-h-0 flex-1 flex-col gap-2">
      <div className="flex items-center justify-between px-1">
        <span className="text-muted-foreground text-xs font-medium tracking-wide uppercase">Workspaces</span>
        <Button size="icon-sm" variant="ghost" title="Add workspace" onClick={addWorkspace} disabled={adding || !online}>
          <FolderPlus />
        </Button>
      </div>
      {error && <p className="text-destructive px-1 text-xs">{error}</p>}
      <div className="flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto" data-testid="workspace-tree">
        {!loaded && <p className="text-muted-foreground px-2 text-xs">Loading…</p>}
        {loaded && tree.length === 0 && (
          <p className="text-muted-foreground px-2 text-xs">No workspaces yet. Add a folder to begin.</p>
        )}
        {tree.map((node) => (
          <WorkspaceRow key={node.workspace.workspace_id} node={node} {...props} onError={setError} />
        ))}
      </div>
    </div>
  );
}

function WorkspaceRow(props: SidebarProps & { node: WorkspaceNode; onError(message: string | null): void }) {
  const controller = useController();
  const online = useOnline();
  const { workspace: w, sessions, working, worktrees } = props.node;
  const isRoot = w.parent_workspace_id === null;
  const selected = props.workspaceId === w.workspace_id;
  const [creating, setCreating] = useState(false);
  const [removing, setRemoving] = useState(false);

  async function newWorktree() {
    props.onError(null);
    setCreating(true);
    try {
      const wt = await controller.createWorktree(w.workspace_id);
      props.onSelectWorkspace(wt.workspace_id);
    } catch (e) {
      props.onError(errorMessage(e));
    } finally {
      setCreating(false);
    }
  }

  return (
    <div data-testid={`workspace-${w.workspace_id}`}>
      <div
        className={cn(
          "group hover:bg-sidebar-accent flex items-center gap-1 rounded-md pr-1",
          selected && "bg-sidebar-accent font-medium",
        )}
      >
        <button
          className="flex min-w-0 flex-1 items-center gap-2 px-2 py-1.5 text-left"
          onClick={() => props.onSelectWorkspace(w.workspace_id)}
        >
          {isRoot ? (
            <FolderGit2 className="text-muted-foreground size-4 shrink-0" />
          ) : (
            <GitBranch className="text-muted-foreground size-4 shrink-0" />
          )}
          <span className="flex min-w-0 flex-col">
            <span className={cn("truncate", w.removed && "text-muted-foreground line-through")}>
              {isRoot ? (w.alias ?? w.name) : worktreeLabel(w)}
            </span>
            {!isRoot && baseLabel(w) && (
              <span className="text-muted-foreground truncate font-mono text-[10px] font-normal">{baseLabel(w)}</span>
            )}
          </span>
          <span className="ml-auto flex shrink-0 items-center gap-1.5 text-[10px] font-normal">
            {isRoot && w.branch && <span className="text-muted-foreground font-mono">{w.branch}</span>}
            {w.removed && <span className="text-muted-foreground">removed</span>}
            {working > 0 && (
              <span className="rounded-full bg-amber-500/15 px-1.5 text-amber-700" title="Agents working">
                {working} working
              </span>
            )}
          </span>
        </button>
        {isRoot && (
          <Button size="icon-sm" variant="ghost" title="New worktree" onClick={newWorktree} disabled={creating || !online}>
            <GitBranchPlus />
          </Button>
        )}
        {!isRoot && !w.removed && (
          <Button
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
      {removing && <RemoveWorktree workspace={w} onClose={() => setRemoving(false)} />}
      <div className="border-sidebar-border ml-4 flex flex-col gap-0.5 border-l py-0.5 pl-2">
        {selected && !w.removed && (
          <button
            className={cn(
              "hover:bg-sidebar-accent text-muted-foreground flex items-center gap-1.5 rounded-md px-2 py-1 text-left text-xs",
              props.sessionId === null && "bg-sidebar-accent text-foreground",
            )}
            onClick={() => props.onSelectSession(w.workspace_id, null)}
          >
            <Plus className="size-3.5" /> New session
          </button>
        )}
        {sessions?.map((s) => (
          <SessionRow
            key={s.session_id}
            session={s}
            selected={selected && props.sessionId === s.session_id}
            onSelect={() => props.onSelectSession(w.workspace_id, s.session_id)}
          />
        ))}
        {worktrees.map((n) => (
          <WorkspaceRow key={n.workspace.workspace_id} {...props} node={n} />
        ))}
      </div>
    </div>
  );
}

function SessionRow(props: { session: SessionSummary; selected: boolean; onSelect(): void }) {
  const s = props.session;
  const view = useHost((st) => st.sessions[s.session_id]);
  const working = isSessionWorking(s, view);
  return (
    <button
      className={cn(
        "hover:bg-sidebar-accent flex items-center gap-1.5 rounded-md px-2 py-1 text-left text-xs",
        props.selected && "bg-sidebar-accent font-medium",
      )}
      onClick={props.onSelect}
      title={`${s.agent ?? "agent"} · ${s.status}`}
    >
      <MessageSquare className="text-muted-foreground size-3.5 shrink-0" />
      <span className="truncate">{s.title || "Untitled session"}</span>
      <span className="text-muted-foreground ml-auto shrink-0 text-[10px]">
        {s.agent ?? "agent"} · {working ? "working" : "idle"}
      </span>
    </button>
  );
}

type RemovePhase =
  | { kind: "confirm" }
  | { kind: "dirty"; changes: number }
  | { kind: "busy"; message: string };

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
    <div className="bg-card my-1 ml-2 flex flex-col gap-2 rounded-md border p-2 text-xs" role="alertdialog">
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
