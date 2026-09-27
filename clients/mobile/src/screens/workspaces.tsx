// Where Files works, picked under its title (DESIGN.md, Workspace context):
// two chips naming the workspace and the worktree, each opening one level of
// the workspace sheet. Level one lists the host's workspaces (search, and New
// workspace to browse the Mac); level two lists a workspace's root checkout
// and worktrees (and New worktree). Choosing a row selects it. A plain folder
// (not a Git repository) has no worktrees: no worktree chip, no level two.

import { useState } from "react";
import { Pressable, ScrollView, StyleSheet, TextInput, View } from "react-native";
import { selectWorkspaceTree, type WorkspaceInfo, type WorkspaceNode } from "@zwiim/sdk";
import { useHostState, useWorkspace } from "../app/hooks";
import { useRootNavigation } from "../app/navigation";
import {
  contextLabel,
  hasWorktreeLevel,
  rootIdOf,
  workspaceLabel,
  workspaceRoots,
  worktreeChoices,
  worktreeError,
} from "../host/workspace";
import { currentHost } from "../services";
import { Button, Chip, Sheet, T } from "../ui/components";
import { ChevronLeft, Icon } from "../ui/icons";
import { radius, space, type, useTheme } from "../ui/tokens";
import { useHostName } from "./HostFrame";

type Level = { kind: "workspaces" } | { kind: "worktrees"; root_id: string; fromList: boolean };

/** `[ zwiim ▾ ]  [ ⎇ fix-auth ▾ ]` for the selected worktree. */
export function ContextBar() {
  const current = useWorkspace();
  const { workspaces } = useHostState();
  const [level, setLevel] = useState<Level | null>(null);
  if (!current) return null;
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
          onPress={() => setLevel({ kind: "worktrees", root_id, fromList: false })}
          testID="worktree-chip"
        />
      ) : null}
      <WorkspaceSheet level={level} onLevel={setLevel} selected={current} />
    </View>
  );
}

/** Opens Browse Mac to add a workspace. */
export function NewWorkspaceButton() {
  const nav = useRootNavigation();
  return <Button kind="secondary" label="New workspace" onPress={() => nav.navigate("BrowseMac")} testID="workspace-new" />;
}

function WorkspaceSheet({ level, onLevel, selected }: { level: Level | null; onLevel(level: Level | null): void; selected: WorkspaceInfo }) {
  const close = () => onLevel(null);
  return (
    <Sheet visible={level !== null} onClose={close}>
      {level?.kind === "workspaces" ? (
        <Workspaces selected={selected} onRoot={(root_id) => onLevel({ kind: "worktrees", root_id, fromList: true })} onClose={close} />
      ) : level?.kind === "worktrees" ? (
        <Worktrees
          root_id={level.root_id}
          selected={selected}
          onBack={level.fromList ? () => onLevel({ kind: "workspaces" }) : undefined}
          onClose={close}
        />
      ) : null}
    </Sheet>
  );
}

function useTree(): WorkspaceNode[] {
  return selectWorkspaceTree(useHostState());
}

/** Level one: the host's workspaces, searchable. */
function Workspaces({ selected, onRoot, onClose }: { selected: WorkspaceInfo; onRoot(root_id: string): void; onClose(): void }) {
  const theme = useTheme();
  const nav = useRootNavigation();
  const tree = useTree();
  const [query, setQuery] = useState("");
  const roots = workspaceRoots(tree, query);
  const selectedRoot = rootIdOf(selected);
  return (
    <>
      <View style={styles.head}>
        <T variant="title" style={styles.flex}>
          Workspaces
        </T>
        <Button
          kind="secondary"
          label="New workspace"
          onPress={() => {
            onClose();
            nav.navigate("BrowseMac");
          }}
          testID="sheet-workspace-new"
        />
      </View>
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
          const worktrees = node.worktrees.filter((n) => !n.workspace.removed).length;
          const working = node.working + node.worktrees.reduce((n, t) => n + t.working, 0);
          const counts = [
            w.is_git_repo ? null : "no Git",
            worktrees ? `${worktrees} ${worktrees === 1 ? "worktree" : "worktrees"}` : null,
            working ? `${working} working` : null,
          ].filter(Boolean);
          // A plain folder is chosen here; a repository opens its worktrees.
          const press = hasWorktreeLevel(node)
            ? () => onRoot(w.workspace_id)
            : () => {
                currentHost.chooseWorkspace(w.workspace_id);
                onClose();
              };
          return (
            <Pressable
              key={w.workspace_id}
              accessibilityRole="button"
              accessibilityState={{ selected: on }}
              onPress={press}
              style={({ pressed }) => [styles.option, (on || pressed) && { backgroundColor: theme.surface }]}
              testID={`workspace-${w.workspace_id}`}
            >
              <View style={styles.optionHead}>
                <View style={[styles.dot, { backgroundColor: on ? theme.accent : "transparent" }]} />
                <T variant="headline" color={on ? theme.accent : theme.fg} numberOfLines={1} style={styles.flex}>
                  {workspaceLabel(w)}
                </T>
                {counts.length ? (
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
            </Pressable>
          );
        })}
      </ScrollView>
    </>
  );
}

/** Level two: a workspace's root checkout and its worktrees. */
function Worktrees({
  root_id,
  selected,
  onBack,
  onClose,
}: {
  root_id: string;
  selected: WorkspaceInfo;
  onBack?: () => void;
  onClose(): void;
}) {
  const theme = useTheme();
  const hostName = useHostName();
  const tree = useTree();
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const choices = worktreeChoices(tree, root_id);
  const root = choices[0]?.workspace;
  if (!root) return null;
  const choose = (id: string) => {
    currentHost.chooseWorkspace(id);
    onClose();
  };
  const create = async () => {
    setCreating(true);
    setError(null);
    try {
      await currentHost.createWorktree(root.workspace_id);
      onClose();
    } catch (e) {
      setError(worktreeError(e, root, hostName));
    } finally {
      setCreating(false);
    }
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
        {choices.map((node, i) => {
          const w = node.workspace;
          const on = w.workspace_id === selected.workspace_id;
          const base = w.base_branch && w.base_commit ? `${w.base_branch}@${w.base_commit.slice(0, 7)}` : w.base_branch ?? w.base_commit?.slice(0, 7);
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
                {i === 0 ? (
                  <T variant="headline" color={on ? theme.accent : theme.fg}>
                    Root
                  </T>
                ) : null}
                <T variant="mono" color={i === 0 ? theme.mut : on ? theme.accent : theme.fg} numberOfLines={1} style={styles.flex}>
                  {i === 0 ? "· " : ""}
                  {i === 0 && !w.is_git_repo ? "no Git" : `⎇ ${w.branch ?? (i === 0 ? "detached" : workspaceLabel(w))}`}
                </T>
                {node.working ? (
                  <T variant="monoSm" color={theme.mut}>
                    {node.working} working
                  </T>
                ) : null}
              </View>
              {i > 0 && base ? (
                <T variant="monoSm" color={theme.mut} numberOfLines={1} style={styles.indent}>
                  from {base}
                </T>
              ) : null}
            </Pressable>
          );
        })}
      </ScrollView>
      {error ? (
        <T color={theme.err} testID="worktree-error">
          {error}
        </T>
      ) : null}
      {root.is_git_repo ? (
        <Button tall kind="secondary" label="New worktree" loading={creating} onPress={() => void create()} testID="worktree-new" />
      ) : (
        <T color={theme.mut} testID="worktree-not-git">
          {workspaceLabel(root)} isn't a Git repository, so it can't have new worktrees.
        </T>
      )}
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
});
