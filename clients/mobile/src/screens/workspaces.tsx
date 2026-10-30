// Where Files works, picked under its title (DESIGN.md, Workspace context):
// two chips naming the workspace and the worktree, each opening one level of
// the workspace sheet. Level one lists the host's workspaces (search, and New
// workspace to browse the Mac); level two lists a workspace's worktrees (and
// New worktree). Never the root itself: the Host runs sessions and terminals
// only in worktrees. Choosing a row selects it; a worktree's Archive asks the
// Host to remove it (RemoveWorktree). A workspace with no worktree opens
// straight into a new one, as Desktop's click does: a plain folder (or one
// with nothing committed) is made a repository first, asking before a large
// first commit. A workspace is removed (RemoveWorkspace) by long-pressing it
// at level one, or from Remove workspace at level two; its folder stays.
//
// New session picks where it starts from the same workspaces as one tree
// (WorkspaceTree): each workspace a tile that opens to its worktrees and New
// worktree, so every choice is one sheet and a tap away.

import { useState } from "react";
import { Pressable, ScrollView, StyleSheet, TextInput, View } from "react-native";
import {
  hasCapability,
  largeFolderDetails,
  largeFolderText,
  selectWorkspaceTree,
  type LargeFolderDetails,
  type WorkspaceInfo,
  type WorkspaceNode,
} from "@zwiim/sdk";
import { useCurrentHost, useHostState, useWorkspace } from "../app/hooks";
import { useRootNavigation } from "../app/navigation";
import {
  contextLabel,
  hasWorktreeLevel,
  openTile,
  rootIdOf,
  workspaceCounts,
  workspaceLabel,
  workspaceRoots,
  worktreeBase,
  worktreeChoices,
  worktreeError,
} from "../host/workspace";
import { prBadge } from "../host/pr";
import { currentHost } from "../services";
import { Button, Chip, Sheet, T } from "../ui/components";
import { ChevronDown, ChevronLeft, ChevronRight, Icon, Plus } from "../ui/icons";
import { fonts, radius, space, type, useTheme } from "../ui/tokens";
import { CloneCards, GithubProjects, NewWorkspaceChooser, useCanClone, useStartedClones } from "./github";
import { useHostName } from "./HostFrame";
import { toneColor } from "./PrStrip";
import { RemoveWorkspace, RemoveWorktree } from "./RemoveWorktree";

type Level =
  | { kind: "workspaces" }
  | { kind: "new" }
  | { kind: "github" }
  | { kind: "worktrees"; root_id: string; fromList: boolean }
  | { kind: "remove"; root_id: string; back: Level };

/** The Host removes whole workspaces (`workspace.remove`). */
function useCanRemoveWorkspace(): boolean {
  const { view } = useCurrentHost();
  return hasCapability(view?.compat ?? null, "workspace.remove");
}

/** `[ zwiim ▾ ]  [ ⎇ fix-auth ▾ ]` for the selected worktree. */
export function ContextBar() {
  const current = useWorkspace();
  const { workspaces, prs } = useHostState();
  const theme = useTheme();
  const [level, setLevel] = useState<Level | null>(null);
  if (!current) return null;
  const pr = prBadge(prs[current.workspace_id]);
  const label = contextLabel(workspaces, current.workspace_id);
  const root_id = rootIdOf(current);
  return (
    <View style={styles.bar}>
      <Chip large label={label?.workspace ?? workspaceLabel(current)} onPress={() => setLevel({ kind: "workspaces" })} testID="workspace-chip" />
      {label?.worktree != null ? (
        <Chip
          large
          glyph="⎇"
          label={label.worktree}
          badge={pr ? { text: pr.text, color: toneColor(pr.tone, theme) } : undefined}
          onPress={() => setLevel({ kind: "worktrees", root_id, fromList: false })}
          testID="worktree-chip"
        />
      ) : null}
      <WorkspaceSheet level={level} onLevel={setLevel} selected={current} />
    </View>
  );
}

/** New workspace from the Files tab: a folder on the Mac, or a GitHub project. */
export function NewWorkspaceButton() {
  const nav = useRootNavigation();
  const canClone = useCanClone();
  const [level, setLevel] = useState<"new" | "github" | null>(null);
  const close = () => setLevel(null);
  // Close only if the sheet is still showing the New workspace flow; a sheet
  // reopened for something else since the clone started stays open.
  const started = useStartedClones(() => {
    if (level === "new" || level === "github") close();
  });
  const folder = () => {
    close();
    nav.navigate("BrowseMac");
  };
  return (
    <>
      <Button kind="secondary" label="New workspace" onPress={canClone ? () => setLevel("new") : folder} testID="workspace-new" />
      <Sheet visible={level !== null} onClose={close}>
        {level === "new" ? (
          <NewWorkspaceChooser onFolder={folder} onGithub={() => setLevel("github")} />
        ) : level === "github" ? (
          <GithubProjects
            onBack={() => setLevel("new")}
            onOpen={(workspace_id) => {
              currentHost.chooseWorkspace(workspace_id);
              close();
            }}
            choose
            onStarted={started}
          />
        ) : null}
      </Sheet>
    </>
  );
}

/** New worktree from the Files tab while none is open: the workspace sheet, where tapping a workspace opens one. */
export function NewWorktreeButton() {
  const [level, setLevel] = useState<Level | null>(null);
  return (
    <>
      <Button kind="secondary" label="New worktree" onPress={() => setLevel({ kind: "workspaces" })} testID="worktree-open" />
      <WorkspaceSheet level={level} onLevel={setLevel} selected={null} />
    </>
  );
}

/**
 * New worktree in a root, as Desktop opens one: a plain folder (or one with
 * nothing committed) is made a repository first; a large first commit is
 * asked about (`large`) and retried with `confirm`.
 */
function useOpenWorktree(choose: boolean, onOpened: (worktree: WorkspaceInfo) => void) {
  const hostName = useHostName();
  const [creating, setCreating] = useState<string | null>(null);
  const [error, setError] = useState<{ root_id: string; text: string } | null>(null);
  const [large, setLarge] = useState<{ root: WorkspaceInfo; details: LargeFolderDetails } | null>(null);
  const open = async (root: WorkspaceInfo, allowLarge = false) => {
    setCreating(root.workspace_id);
    setError(null);
    try {
      const worktree = await currentHost.openWorktree(root.workspace_id, { choose, allowLarge });
      setLarge(null);
      onOpened(worktree);
    } catch (e) {
      const details = allowLarge ? null : largeFolderDetails(e);
      setLarge(details ? { root, details } : null);
      if (!details) setError({ root_id: root.workspace_id, text: worktreeError(e, root, hostName) });
    } finally {
      setCreating(null);
    }
  };
  return {
    open: (root: WorkspaceInfo) => void open(root),
    confirm: () => void (large && open(large.root, true)),
    cancel: () => setLarge(null),
    creating,
    error,
    large,
  };
}

/** Desktop's Large folder confirm, before the first commit of a plain folder. */
export function LargeFolder({
  root,
  details,
  adding,
  onCancel,
  onAdd,
}: {
  root: WorkspaceInfo;
  details: LargeFolderDetails;
  adding: boolean;
  onCancel(): void;
  onAdd(): void;
}) {
  const theme = useTheme();
  return (
    <View style={styles.confirm} testID="worktree-large">
      <T variant="title">Large folder</T>
      <T color={theme.mut}>
        {workspaceLabel(root)} isn't a Git repository yet. Zwiim will commit{" "}
        <T color={theme.mut} style={styles.strong}>
          {largeFolderText(details)}
        </T>{" "}
        as "Initial Commit". Add it anyway?
      </T>
      <View style={styles.optionHead}>
        <View style={styles.flex}>
          <Button tall kind="secondary" label="Cancel" disabled={adding} onPress={onCancel} testID="worktree-large-cancel" />
        </View>
        <View style={styles.flex}>
          <Button tall label="Add anyway" loading={adding} onPress={onAdd} testID="worktree-large-add" />
        </View>
      </View>
    </View>
  );
}

function WorkspaceSheet({ level, onLevel, selected }: { level: Level | null; onLevel(level: Level | null): void; selected: WorkspaceInfo | null }) {
  const nav = useRootNavigation();
  const canClone = useCanClone();
  const close = () => onLevel(null);
  // CurrentHost selects a done clone's worktree; close only if the sheet is
  // still showing the New workspace flow (not reopened for something else).
  const started = useStartedClones(() => {
    if (level?.kind === "new" || level?.kind === "github") close();
  });
  const folder = () => {
    close();
    nav.navigate("BrowseMac");
  };
  return (
    <Sheet visible={level !== null} onClose={close}>
      {level?.kind === "workspaces" ? (
        <Workspaces
          selected={selected}
          onRoot={(root_id) => onLevel({ kind: "worktrees", root_id, fromList: true })}
          onRemove={(root_id) => onLevel({ kind: "remove", root_id, back: level })}
          onNew={canClone ? () => onLevel({ kind: "new" }) : folder}
          onStarted={started}
          onClose={close}
        />
      ) : level?.kind === "new" ? (
        <NewWorkspaceChooser onFolder={folder} onGithub={() => onLevel({ kind: "github" })} />
      ) : level?.kind === "github" ? (
        <GithubProjects
          onBack={() => onLevel({ kind: "new" })}
          onOpen={(root_id) => onLevel({ kind: "worktrees", root_id, fromList: true })}
          choose
          onStarted={started}
        />
      ) : level?.kind === "worktrees" ? (
        <Worktrees
          root_id={level.root_id}
          selected={selected}
          onBack={level.fromList ? () => onLevel({ kind: "workspaces" }) : undefined}
          onRemove={() => onLevel({ kind: "remove", root_id: level.root_id, back: level })}
          onClose={close}
        />
      ) : level?.kind === "remove" ? (
        <RemoveRoot root_id={level.root_id} onDone={close} onCancel={() => onLevel(level.back)} />
      ) : null}
    </Sheet>
  );
}

function useTree(): WorkspaceNode[] {
  return selectWorkspaceTree(useHostState());
}

/** A workspace's removal, in the sheet; nothing once the Host no longer lists it. */
function RemoveRoot({ root_id, onDone, onCancel }: { root_id: string; onDone(): void; onCancel(): void }) {
  const root = useTree().find((n) => n.workspace.workspace_id === root_id)?.workspace;
  if (!root) return null;
  return <RemoveWorkspace workspace={root} onDone={onDone} onCancel={onCancel} />;
}

/** Level one: the host's workspaces, searchable. Long-press removes one. */
function Workspaces({
  selected,
  onRoot,
  onRemove,
  onNew,
  onStarted,
  onClose,
}: {
  selected: WorkspaceInfo | null;
  onRoot(root_id: string): void;
  onRemove(root_id: string): void;
  onNew(): void;
  onStarted(clone_id: string): void;
  onClose(): void;
}) {
  const theme = useTheme();
  const tree = useTree();
  const canRemove = useCanRemoveWorkspace();
  const [query, setQuery] = useState("");
  const roots = workspaceRoots(tree, query);
  const selectedRoot = selected ? rootIdOf(selected) : null;
  const worktree = useOpenWorktree(true, onClose);
  if (worktree.large) {
    return (
      <LargeFolder
        {...worktree.large}
        adding={worktree.creating !== null}
        onCancel={worktree.cancel}
        onAdd={worktree.confirm}
      />
    );
  }
  return (
    <>
      <View style={styles.head}>
        <T variant="title" style={styles.flex}>
          Workspaces
        </T>
        <Button kind="secondary" label="New workspace" onPress={onNew} testID="sheet-workspace-new" />
      </View>
      <CloneCards onStarted={onStarted} />
      <TextInput
        value={query}
        onChangeText={setQuery}
        placeholder="Search workspaces"
        placeholderTextColor={theme.faint}
        autoCapitalize="none"
        autoCorrect={false}
        clearButtonMode="while-editing"
        style={[type.body, styles.search, { color: theme.fg, backgroundColor: theme.surface }]}
        accessibilityLabel="Search workspaces"
        testID="workspace-search"
      />
      <ScrollView keyboardShouldPersistTaps="handled">
        {roots.length === 0 ? (
          <T color={theme.mut} style={styles.none}>
            No workspace matches “{query.trim()}”.
          </T>
        ) : null}
        {roots.map((node) => {
          const w = node.workspace;
          const on = w.workspace_id === selectedRoot;
          const counts = workspaceCounts(node);
          const creating = worktree.creating === w.workspace_id;
          // One with worktrees opens them; one with none opens a new one.
          const press = hasWorktreeLevel(node) ? () => onRoot(w.workspace_id) : () => worktree.open(w);
          return (
            <Pressable
              key={w.workspace_id}
              accessibilityRole="button"
              accessibilityState={{ selected: on }}
              accessibilityActions={canRemove ? [{ name: "remove", label: "Remove workspace" }] : undefined}
              onAccessibilityAction={(e) => {
                if (e.nativeEvent.actionName === "remove") onRemove(w.workspace_id);
              }}
              disabled={worktree.creating !== null}
              onPress={press}
              onLongPress={canRemove ? () => onRemove(w.workspace_id) : undefined}
              style={({ pressed }) => [styles.option, (on || pressed) && { backgroundColor: theme.surface }]}
              testID={`workspace-${w.workspace_id}`}
            >
              <View style={styles.optionHead}>
                <View style={[styles.dot, { backgroundColor: on ? theme.accent : "transparent" }]} />
                <T variant="headline" color={on ? theme.accent : theme.fg} numberOfLines={1} style={styles.flex}>
                  {workspaceLabel(w)}
                </T>
                {creating ? (
                  <T variant="monoSm" color={theme.mut}>
                    Creating worktree…
                  </T>
                ) : counts.length ? (
                  <T variant="monoSm" color={theme.mut}>
                    {counts.join(" · ")}
                  </T>
                ) : null}
              </View>
              {w.display_path ? (
                <T variant="monoSm" color={theme.mut} numberOfLines={1} ellipsizeMode="head" style={styles.indent}>
                  {w.display_path}
                </T>
              ) : null}
              {worktree.error?.root_id === w.workspace_id ? (
                <T color={theme.err} style={styles.indent} testID="workspace-worktree-error">
                  {worktree.error.text}
                </T>
              ) : null}
            </Pressable>
          );
        })}
      </ScrollView>
    </>
  );
}

/** Level two: a workspace's worktrees, and New worktree. */
function Worktrees({
  root_id,
  selected,
  onBack,
  onRemove,
  onClose,
}: {
  root_id: string;
  selected: WorkspaceInfo | null;
  onBack?: () => void;
  onRemove(): void;
  onClose(): void;
}) {
  const theme = useTheme();
  const tree = useTree();
  const canRemove = useCanRemoveWorkspace();
  const [removing, setRemoving] = useState<WorkspaceInfo | null>(null);
  const worktree = useOpenWorktree(true, onClose);
  const choices = worktreeChoices(tree, root_id);
  const root = tree.find((n) => n.workspace.workspace_id === root_id)?.workspace;
  if (!root) return null;
  if (removing) return <RemoveWorktree worktree={removing} onDone={onClose} onCancel={() => setRemoving(null)} />;
  if (worktree.large) {
    return (
      <LargeFolder {...worktree.large} adding={worktree.creating !== null} onCancel={worktree.cancel} onAdd={worktree.confirm} />
    );
  }
  const choose = (id: string) => {
    currentHost.chooseWorkspace(id);
    onClose();
  };
  return (
    <>
      <View style={styles.head}>
        {onBack ? (
          <Pressable accessibilityRole="button" accessibilityLabel="Workspaces" onPress={onBack} hitSlop={8} testID="worktrees-back">
            <Icon icon={ChevronLeft} color={theme.fg} size={24} />
          </Pressable>
        ) : null}
        <T variant="title" numberOfLines={1} style={styles.flex}>
          {workspaceLabel(root)}
        </T>
      </View>
      <ScrollView>
        {choices.map((node) => {
          const w = node.workspace;
          const on = w.workspace_id === selected?.workspace_id;
          const base = worktreeBase(w);
          return (
            <Pressable
              key={w.workspace_id}
              accessibilityRole="radio"
              accessibilityState={{ checked: on }}
              onPress={() => choose(w.workspace_id)}
              style={({ pressed }) => [styles.option, (on || pressed) && { backgroundColor: theme.surface }]}
              testID={`worktree-${w.workspace_id}`}
            >
              <View style={styles.optionHead}>
                <View style={[styles.dot, { backgroundColor: on ? theme.accent : "transparent" }]} />
                <T variant="mono" color={on ? theme.accent : theme.fg} numberOfLines={1} style={styles.flex}>
                  ⎇ {w.branch ?? workspaceLabel(w)}
                </T>
                {node.working ? (
                  <T variant="monoSm" color={theme.mut}>
                    {node.working} working
                  </T>
                ) : null}
                <Pressable
                  accessibilityRole="button"
                  accessibilityLabel={`Archive ${w.branch ?? workspaceLabel(w)}`}
                  onPress={() => setRemoving(w)}
                  hitSlop={8}
                  testID={`worktree-remove-${w.workspace_id}`}
                >
                  <T variant="callout" color={theme.mut}>
                    Archive
                  </T>
                </Pressable>
              </View>
              {base ? (
                <T variant="monoSm" color={theme.mut} numberOfLines={1} style={styles.indent}>
                  from {base}
                </T>
              ) : null}
            </Pressable>
          );
        })}
      </ScrollView>
      {worktree.error ? (
        <T color={theme.err} testID="worktree-error">
          {worktree.error.text}
        </T>
      ) : null}
      <Button
        tall
        kind="secondary"
        label="New worktree"
        loading={worktree.creating !== null}
        onPress={() => worktree.open(root)}
        testID="worktree-new"
      />
      {canRemove ? (
        <Button kind="ghost" color={theme.err} label="Remove workspace" onPress={onRemove} testID="workspace-remove" />
      ) : null}
    </>
  );
}

/**
 * Where a new session starts, as one tree: each workspace a tile that opens
 * to its worktrees (and New worktree), never its root. The tile holding
 * `selected` starts open, else the first. Choosing calls `onChoose`; it
 * doesn't change the Files selection. Without New workspace (switching only),
 * a workspace with no worktree isn't listed.
 */
export function WorkspaceTree({
  title = "Workspace",
  selected,
  onChoose,
  onNewWorkspace,
}: {
  title?: string;
  selected: string | null;
  onChoose(workspace_id: string): void;
  /** Without it the tree only switches: no New workspace, no New worktree. */
  onNewWorkspace?(): void;
}) {
  const theme = useTheme();
  const hostName = useHostName();
  const tree = useTree();
  const [query, setQuery] = useState("");
  const [open, setOpen] = useState<ReadonlySet<string>>(() => new Set([openTile(tree, selected)].filter((id) => id !== null)));
  // Files keeps its own selection: the new worktree is only this choice.
  const worktree = useOpenWorktree(false, (w) => onChoose(w.workspace_id));
  const roots = workspaceRoots(tree, query).filter((n) => onNewWorkspace || hasWorktreeLevel(n));

  const toggle = (id: string) =>
    setOpen((was) => {
      const next = new Set(was);
      if (!next.delete(id)) next.add(id);
      return next;
    });
  if (worktree.large) {
    return (
      <LargeFolder {...worktree.large} adding={worktree.creating !== null} onCancel={worktree.cancel} onAdd={worktree.confirm} />
    );
  }
  const creating = worktree.creating;
  const error = worktree.error;

  return (
    <>
      <View style={styles.head}>
        <T variant="title" style={styles.flex}>
          {title}
        </T>
        {onNewWorkspace ? <Button kind="secondary" label="New workspace" onPress={onNewWorkspace} testID="tree-workspace-new" /> : null}
      </View>
      {tree.length > 1 ? (
        <TextInput
          value={query}
          onChangeText={setQuery}
          placeholder="Search workspaces"
          placeholderTextColor={theme.faint}
          autoCapitalize="none"
          autoCorrect={false}
          clearButtonMode="while-editing"
          style={[type.body, styles.search, { color: theme.fg, backgroundColor: theme.surface }]}
          accessibilityLabel="Search workspaces"
          testID="tree-search"
        />
      ) : null}
      <ScrollView keyboardShouldPersistTaps="handled">
        {roots.length === 0 ? (
          <T color={theme.mut} style={styles.none}>
            {query.trim()
              ? `No workspace matches “${query.trim()}”.`
              : onNewWorkspace
                ? `No workspaces on ${hostName} yet. Add a folder with New workspace.`
                : `No worktrees on ${hostName} yet.`}
          </T>
        ) : null}
        {roots.map((node) => {
          const w = node.workspace;
          const choices = worktreeChoices(tree, w.workspace_id);
          const holds = choices.some((c) => c.workspace.workspace_id === selected);
          const counts = workspaceCounts(node);
          const expanded = open.has(w.workspace_id);
          return (
            <View key={w.workspace_id}>
              <Pressable
                accessibilityRole="button"
                accessibilityState={{ expanded }}
                onPress={() => toggle(w.workspace_id)}
                style={({ pressed }) => [styles.option, pressed && { backgroundColor: theme.surface }]}
                testID={`tree-${w.workspace_id}`}
              >
                <View style={styles.optionHead}>
                  <View style={styles.lead}>
                    <Icon icon={expanded ? ChevronDown : ChevronRight} color={theme.mut} size={16} />
                  </View>
                  <T variant="headline" color={holds ? theme.accent : theme.fg} numberOfLines={1} style={styles.flex}>
                    {workspaceLabel(w)}
                  </T>
                  {counts.length ? (
                    <T variant="monoSm" color={theme.mut}>
                      {counts.join(" · ")}
                    </T>
                  ) : null}
                </View>
                {w.display_path ? (
                  <T variant="monoSm" color={theme.mut} numberOfLines={1} ellipsizeMode="head" style={styles.treeIndent}>
                    {w.display_path}
                  </T>
                ) : null}
              </Pressable>
              {expanded ? (
                <View style={[styles.branch, { borderLeftColor: theme.line }]}>
                  {choices.map((child) => {
                    const c = child.workspace;
                    const on = c.workspace_id === selected;
                    const base = worktreeBase(c);
                    return (
                      <Pressable
                        key={c.workspace_id}
                        accessibilityRole="radio"
                        accessibilityState={{ checked: on }}
                        onPress={() => onChoose(c.workspace_id)}
                        style={({ pressed }) => [styles.leaf, (on || pressed) && { backgroundColor: theme.surface }]}
                        testID={`tree-leaf-${c.workspace_id}`}
                      >
                        <View style={styles.optionHead}>
                          <View style={[styles.dot, { backgroundColor: on ? theme.accent : "transparent" }]} />
                          <T variant="mono" color={on ? theme.accent : theme.fg} numberOfLines={1} style={styles.flex}>
                            ⎇ {c.branch ?? workspaceLabel(c)}
                          </T>
                          {child.working ? (
                            <T variant="monoSm" color={theme.mut}>
                              {child.working} working
                            </T>
                          ) : null}
                        </View>
                        {base ? (
                          <T variant="monoSm" color={theme.mut} numberOfLines={1} style={styles.indent}>
                            from {base}
                          </T>
                        ) : null}
                      </Pressable>
                    );
                  })}
                  {onNewWorkspace ? (
                    <Pressable
                      accessibilityRole="button"
                      disabled={creating !== null}
                      onPress={() => worktree.open(w)}
                      style={({ pressed }) => [styles.leaf, pressed && { backgroundColor: theme.surface }]}
                      testID={`tree-new-worktree-${w.workspace_id}`}
                    >
                      <View style={styles.optionHead}>
                        <Icon icon={Plus} color={creating === w.workspace_id ? theme.faint : theme.accent} size={16} />
                        <T variant="headline" color={creating === w.workspace_id ? theme.faint : theme.accent}>
                          {creating === w.workspace_id ? "Creating worktree…" : "New worktree"}
                        </T>
                      </View>
                    </Pressable>
                  ) : null}
                  {error?.root_id === w.workspace_id ? (
                    <T color={theme.err} style={styles.leafNote} testID="tree-worktree-error">
                      {error.text}
                    </T>
                  ) : null}
                </View>
              ) : null}
            </View>
          );
        })}
      </ScrollView>
    </>
  );
}

const styles = StyleSheet.create({
  flex: { flex: 1 },
  bar: { flexDirection: "row", flexWrap: "wrap", gap: space.sm, paddingHorizontal: space.lg, paddingTop: space.md, paddingBottom: space.sm },
  head: { flexDirection: "row", alignItems: "center", gap: space.sm },
  search: { height: 44, borderRadius: radius.md, paddingHorizontal: space.md },
  none: { paddingVertical: space.md },
  option: { minHeight: 52, borderRadius: radius.md, paddingHorizontal: space.md, paddingVertical: space.sm, gap: space.xxs, justifyContent: "center" },
  optionHead: { flexDirection: "row", alignItems: "center", gap: space.sm },
  dot: { width: 8, height: 8, borderRadius: radius.full },
  indent: { paddingLeft: space.lg },
  // The tree: a tile's lead (chevron or dot) is 16 wide; its children hang on a line under it.
  lead: { width: 16, alignItems: "center" },
  treeIndent: { paddingLeft: 16 + space.sm },
  branch: { marginLeft: space.md + space.sm, paddingLeft: space.xs, borderLeftWidth: StyleSheet.hairlineWidth },
  leaf: { minHeight: 44, borderRadius: radius.md, paddingHorizontal: space.sm, paddingVertical: space.xs, gap: space.xxs, justifyContent: "center" },
  leafNote: { paddingHorizontal: space.sm, paddingVertical: space.xs },
  confirm: { gap: space.sm },
  strong: { fontFamily: fonts.sansSemi },
});
