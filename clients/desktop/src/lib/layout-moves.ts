import { useEffect, useMemo, useRef, useState, type DragEvent } from "react";
import {
  buildSidebarTree,
  groupOf,
  moveInLayout,
  placeOf,
  stepTarget,
  type LayoutRef,
  type LayoutTarget,
  type SidebarEntry,
  type WorkspaceGroup,
  type WorkspaceLayout,
} from "@zwiim/sdk";
import { errorMessage, useController, useHost, useOnline } from "@/lib/host-context";

/** How long an answered move shows before the Host's layout, which normally ends it sooner. */
export const MOVE_SETTLE_MS = 3000;

/** The sidebar's note row. `undo` puts a move back; `alert` is an error. */
export interface SidebarNote {
  text: string;
  undo?: () => void;
  alert?: boolean;
}

/** Where a drag over a row lands, `at` its height from the row's top; null leaves it to the row around this one. */
export type DropZone = (source: LayoutRef, at: number) => LayoutTarget | null;

const edge = (ref: LayoutRef, at: number): LayoutTarget => ({ at: at < 0.5 ? "before" : "after", ref });

export function worktreeZone(ref: LayoutRef): DropZone {
  return (source, at) => (source.kind === "worktree" ? edge(ref, at) : null);
}

/** Anywhere over a repository, its worktrees too. A group passes over members to the group holding them. */
export function repositoryZone(ref: LayoutRef, grouped: boolean): DropZone {
  return (source, at) =>
    source.kind === "worktree" || (source.kind === "group" && grouped) ? null : edge(ref, at);
}

export function groupZone(group_id: string, firstMember: LayoutRef | null): DropZone {
  const ref: LayoutRef = { kind: "group", id: group_id };
  return (source, at) => {
    if (source.kind === "worktree") return null;
    if (source.kind === "group") return edge(ref, at);
    if (at < 0.25) return { at: "before", ref };
    if (at < 0.75 || at > 1) return { at: "into", group_id };
    // Just under an open group's row is the top of its members.
    return firstMember ? { at: "before", ref: firstMember } : { at: "after", ref };
  };
}

export const endZone: DropZone = (source) => (source.kind === "worktree" ? null : { at: "end" });

/** Which side of `ref` the insertion line sits on, if there. */
export function lineAt(over: LayoutTarget | null, ref: LayoutRef): "before" | "after" | null {
  if (!over || over.at === "into" || over.at === "end") return null;
  return over.ref.kind === ref.kind && over.ref.id === ref.id ? over.at : null;
}

function targetGone(layout: WorkspaceLayout, target: LayoutTarget): boolean {
  if (target.at === "end") return false;
  if (target.at === "into") return !layout.entries.some((e) => e.kind === "group" && e.group_id === target.group_id);
  return placeOf(layout, target.ref) === null;
}

const groupName = (layout: WorkspaceLayout | null, id: string | null) => {
  const entry = layout?.entries.find((e) => e.kind === "group" && e.group_id === id);
  return entry?.kind === "group" ? entry.name : undefined;
};

/** A target's name, for saying what became of it. */
const nameOf = (layout: WorkspaceLayout | null, target: LayoutTarget) =>
  target.at === "into" ? groupName(layout, target.group_id) : undefined;

function moved(label: string, before: WorkspaceLayout, after: WorkspaceLayout, source: LayoutRef): string {
  const from = groupOf(before, source);
  const to = groupOf(after, source);
  if (from === to) return `Moved ${label}`;
  return to ? `Moved ${label} to ${groupName(after, to)}` : `Moved ${label} out of ${groupName(before, from)}`;
}

interface MoveOptions {
  /** The target's name, to say so if it went away. */
  targetName?: string;
  /** Focus follows the row: a menu closed, or Undo left. */
  focus?: boolean;
  /** A group the Host just made, which the layout may not list yet. */
  created?: WorkspaceGroup;
  undoing?: boolean;
  /** The group made for the move being undone, which goes too unless something else moved in. */
  made?: string;
}

/**
 * Every way to move something in the sidebar. Each move is worked out on the
 * latest layout when it happens, shows at once, and ends with the first
 * layout after the Host answers: earlier ones don't have it in them. Nothing
 * queues: offline or mid-write there is no move.
 */
export function useLayoutMoves(args: {
  entries: SidebarEntry[];
  groupable: boolean;
  onNote(note: SidebarNote | null): void;
  onUnfold(group_id: string): void;
}) {
  const { entries, groupable, onNote, onUnfold } = args;
  const controller = useController();
  const online = useOnline();
  const hostLayout = useHost((s) => s.layout);
  const [write, setWrite] = useState<{ layout: WorkspaceLayout; answered: boolean } | null>(null);
  const sending = useRef(false);
  const [over, setOver] = useState<LayoutTarget | null>(null);
  const dragging = useRef<{ source: LayoutRef; label: string } | null>(null);
  const refocus = useRef<string | null>(null);

  const pending = write?.layout ?? null;
  const layout = pending ?? hostLayout;
  const canMove = groupable && hostLayout !== null;
  const canDrag = canMove && online && pending === null;
  const shown = useMemo(
    () => (pending ? buildSidebarTree({ ...controller.store.getState(), layout: pending }) : entries),
    [pending, entries],
  );

  useEffect(() => setWrite((was) => (was?.answered ? null : was)), [hostLayout]);

  useEffect(() => {
    if (!write?.answered) return;
    const timer = setTimeout(() => setWrite(null), MOVE_SETTLE_MS);
    return () => clearTimeout(timer);
  }, [write]);

  useEffect(() => {
    if (!online) endDrag();
  }, [online]);

  useEffect(() => {
    const id = refocus.current;
    if (id === null) return;
    refocus.current = null;
    document.querySelectorAll<HTMLElement>("[data-row]").forEach((row) => {
      if (row.dataset.row === id) row.focus();
    });
  }, [shown]);

  function endDrag() {
    dragging.current = null;
    setOver(null);
  }

  /** Whether a write went out. One that failed has said so in the note row. */
  async function move(source: LayoutRef, label: string, target: LayoutTarget, options: MoveOptions = {}) {
    const latest = controller.latestLayout();
    if (!online || sending.current || pending || !latest) return false;
    const { created } = options;
    const base =
      created && !latest.entries.some((e) => e.kind === "group" && e.group_id === created.group_id)
        ? { entries: [...latest.entries, { kind: "group" as const, ...created, workspaces: [] }] }
        : latest;
    const next = moveInLayout(base, source, target);
    if (!next) {
      const stayed = `${label} stayed put.`;
      const text = options.targetName ? `${options.targetName} was deleted on another device. ${stayed}` : stayed;
      onNote(targetGone(base, target) ? { text } : null);
      return false;
    }
    const from = placeOf(base, source);
    const home = groupOf(next, source);
    onNote(null);
    if (home) onUnfold(home);
    if (options.focus) refocus.current = source.id;
    sending.current = true;
    setWrite({ layout: next, answered: false });
    try {
      await controller.setWorkspaceLayout(next);
    } catch (e) {
      setWrite(null);
      onNote({ text: errorMessage(e), alert: true });
      return true;
    } finally {
      sending.current = false;
    }
    // The feed can beat the answer, and then no layout is coming.
    const arrived = JSON.stringify(controller.store.getState().layout) === JSON.stringify(next);
    setWrite(arrived ? null : { layout: next, answered: true });
    const made = next.entries.find((e) => e.kind === "group" && e.group_id === options.made);
    if (made?.kind === "group" && made.workspaces.length === 0) {
      await controller.deleteWorkspaceGroup(made.group_id).catch((e) => onNote({ text: errorMessage(e), alert: true }));
    }
    if (options.undoing || !from) return true;
    const targetName = nameOf(base, from);
    onNote({
      text: moved(label, base, next, source),
      undo: () =>
        void latestMove.current(source, label, from, {
          targetName,
          focus: true,
          undoing: true,
          made: created?.group_id,
        }),
    });
    return true;
  }
  // Undo outlives the render that offered it.
  const latestMove = useRef(move);
  latestMove.current = move;

  return {
    /** The layout as the sidebar shows it, and the tree drawn from it. */
    layout,
    entries: shown,
    groups: (layout?.entries ?? []).flatMap((e) =>
      e.kind === "group" ? [{ group_id: e.group_id, name: e.name }] : [],
    ),
    canMove,
    canDrag,
    /** Where the drag in progress would land. */
    over,
    move,
    /**
     * Make a group and move into it, two writes. It makes nothing mid-write,
     * where the move could only be dropped: the name stays to try again.
     */
    async moveToNewGroup(source: LayoutRef, label: string, name: string) {
      if (sending.current || pending) throw new Error("Wait for the move on its way, then try again.");
      const created = await controller.createWorkspaceGroup(name);
      const target = { at: "into", group_id: created.group_id } as const;
      // A lot can change during the create, so this is the latest render's move.
      const sent = await latestMove.current(source, label, target, { created, focus: true });
      if (!sent) onNote({ text: `${label} stayed put. ${created.name} is empty.` });
    },
    canStep: (source: LayoutRef, by: -1 | 1) => (layout ? stepTarget(layout, source, by) !== null : false),
    /** Move up or down, from a menu. */
    step(source: LayoutRef, label: string, by: -1 | 1) {
      const latest = controller.latestLayout();
      const target = latest && stepTarget(latest, source, by);
      if (target) void move(source, label, target, { focus: true });
    },
    dragProps(source: LayoutRef, label: string) {
      return {
        draggable: canDrag,
        onDragStart(e: DragEvent<HTMLElement>) {
          dragging.current = { source, label };
          // WebKit starts no drag without data. Over a row only `dragging` is read.
          e.dataTransfer.setData("text/plain", label);
          e.dataTransfer.effectAllowed = "move";
        },
        onDragEnd: endDrag,
      };
    },
    dropProps(zone: DropZone) {
      const targetOf = (e: DragEvent<HTMLElement>, source: LayoutRef) => {
        const row = (e.currentTarget.firstElementChild ?? e.currentTarget).getBoundingClientRect();
        return zone(source, (e.clientY - row.top) / row.height);
      };
      return {
        onDragOver(e: DragEvent<HTMLElement>) {
          const source = dragging.current?.source;
          const target = source && targetOf(e, source);
          if (!source || !target) return;
          e.stopPropagation();
          const latest = controller.latestLayout();
          if (!latest || !moveInLayout(latest, source, target)) return setOver(null);
          e.preventDefault();
          e.dataTransfer.dropEffect = "move";
          setOver((was) => (JSON.stringify(was) === JSON.stringify(target) ? was : target));
        },
        onDrop(e: DragEvent<HTMLElement>) {
          const drag = dragging.current;
          const target = drag && targetOf(e, drag.source);
          if (!drag || !target) return;
          e.preventDefault();
          e.stopPropagation();
          // The row may leave with the move, and WebKit then sends it no dragend.
          endDrag();
          void move(drag.source, drag.label, target, { targetName: nameOf(layout, target) });
        },
      };
    },
    /** Over no row that takes this drag. */
    clearOver: () => setOver(null),
  };
}

export type LayoutMoves = ReturnType<typeof useLayoutMoves>;
