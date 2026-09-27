// The Files tab (DESIGN.md, Screen 6): inspection, not editing. Changes lists
// what differs from the last commit (`git.status`, `git.diff`) and opens each
// file's diff; Browse walks the worktree one folder at a time
// (`workspace.tree`) and opens files read-only; Terminals lists and starts its
// terminals. All three follow the worktree chosen in the context bar. A plain
// folder (not a Git repository) has no changes: Changes says so, unasked.

import { useCallback, useEffect, useRef, useState } from "react";
import { BackHandler, Pressable, ScrollView, StyleSheet, View } from "react-native";
import { useFocusEffect } from "@react-navigation/native";
import type { FileTreeNode, WorkspaceInfo } from "@zwiim/sdk";
import { useHostRead, useHostState, useWorkspace, type Loaded } from "../app/hooks";
import { useRootNavigation } from "../app/navigation";
import { changeItems, crumbs, fileProblem, folderEntries, notGitProblem, parentPath, type ChangeItem } from "../files/model";
import { workspaceLabel } from "../host/workspace";
import { currentHost } from "../services";
import { Button, EmptyState, Segmented, SkeletonRows, T } from "../ui/components";
import { radius, space, useTheme } from "../ui/tokens";
import { HostFrame, useHostName } from "./HostFrame";
import { TerminalsSegment } from "./TerminalScreens";
import { ContextBar, NewWorkspaceButton } from "./workspaces";

type Mode = "changes" | "browse" | "terminals";

/** Read again when the tab is shown again (not on first show: it has just been read). */
function useRefocus(reload: () => void) {
  const first = useRef(true);
  useFocusEffect(
    useCallback(() => {
      if (first.current) first.current = false;
      else reload();
    }, [reload]),
  );
}

export function FilesTab() {
  const workspace = useWorkspace();
  const name = useHostName();
  // Kept across worktrees: switching shows the same segment of the next one.
  const [chosen, setChosen] = useState<Mode | null>(null);
  return (
    <HostFrame title="Files" accessory={workspace ? <NewWorkspaceButton /> : null}>
      {workspace ? (
        <>
          <ContextBar />
          <Files key={workspace.workspace_id} workspace={workspace} chosen={chosen} onChoose={setChosen} />
        </>
      ) : (
        <EmptyState
          glyph="/"
          title="No workspaces yet"
          body={`Add a folder on ${name} to work in it from here.`}
          action={<NewWorkspaceButton />}
        />
      )}
    </HostFrame>
  );
}

function Files({ workspace, chosen, onChoose }: { workspace: WorkspaceInfo; chosen: Mode | null; onChoose(mode: Mode): void }) {
  const name = useHostName();
  const id = workspace.workspace_id;
  const revision = useHostState().diffRevision[id] ?? 0;
  const [path, setPath] = useState("");

  // The host bumps the revision when the workspace's files change on disk.
  // Git is never asked about a plain folder.
  const changes = useHostRead(workspace.is_git_repo ? `${id}:${revision}` : null, async () => {
    const [status, diff] = await Promise.all([
      currentHost.request("git.status", { workspace_id: id, show_all_untracked: true }),
      currentHost.request("git.diff", { workspace_id: id }),
    ]);
    return { items: changeItems(status, diff), truncated: diff.truncated };
  });
  const tree = useHostRead(`${id}:${path}`, () =>
    currentHost.request("workspace.tree", path ? { workspace_id: id, path, max_depth: 1 } : { workspace_id: id, max_depth: 1 }),
  );
  useRefocus(
    useCallback(() => {
      changes.reload();
      tree.reload();
    }, [changes.reload, tree.reload]),
  );
  // Files changing on disk re-read the folder shown, keeping it on screen meanwhile.
  const reloadTree = tree.reload;
  const seen = useRef(revision);
  useEffect(() => {
    if (seen.current === revision) return;
    seen.current = revision;
    reloadTree();
  }, [revision, reloadTree]);

  // Changes first when there are any; Browse for a clean or non-Git workspace.
  const settled = !workspace.is_git_repo
    ? "browse"
    : changes.data
      ? changes.data.items.length
        ? "changes"
        : "browse"
      : changes.error
        ? "browse"
        : null;
  const mode = chosen ?? settled;

  // Android back climbs out of a folder before leaving the tab.
  useFocusEffect(
    useCallback(() => {
      if (mode !== "browse" || !path) return;
      const sub = BackHandler.addEventListener("hardwareBackPress", () => {
        setPath(parentPath(path));
        return true;
      });
      return () => sub.remove();
    }, [mode, path]),
  );

  return (
    <View>
      <View style={styles.segmented}>
        <Segmented<Mode>
          options={[
            { key: "changes", label: "Changes" },
            { key: "browse", label: "Browse" },
            { key: "terminals", label: "Terminals" },
          ]}
          value={mode ?? "changes"}
          onChange={onChoose}
        />
      </View>
      {mode === null ? (
        <SkeletonRows />
      ) : mode === "changes" ? (
        <Changes workspace={workspace} loaded={changes} hostName={name} onBrowse={() => onChoose("browse")} />
      ) : mode === "terminals" ? (
        <TerminalsSegment workspace={workspace} />
      ) : (
        <Browse workspace={workspace} path={path} onPath={setPath} loaded={tree} hostName={name} />
      )}
    </View>
  );
}

function Changes({
  workspace,
  loaded,
  hostName,
  onBrowse,
}: {
  workspace: WorkspaceInfo;
  loaded: Loaded<{ items: ChangeItem[]; truncated: boolean }>;
  hostName: string;
  onBrowse(): void;
}) {
  const theme = useTheme();
  const nav = useRootNavigation();
  const branch = workspace.branch ?? "detached HEAD";
  if (!workspace.is_git_repo) {
    const p = notGitProblem(workspace);
    return <EmptyState glyph="⎇" title={p.title} body={p.body} action={<Button kind="secondary" label="Browse files" onPress={onBrowse} />} />;
  }
  if (loaded.error) {
    const p = fileProblem(loaded.error, hostName, "changes");
    return <EmptyState glyph="⎇" title={p.title} body={p.body} action={<Button kind="secondary" label="Try again" onPress={loaded.reload} />} />;
  }
  if (!loaded.data) return <SkeletonRows />;
  const { items, truncated } = loaded.data;
  return (
    <View>
      <View style={[styles.branch, { backgroundColor: theme.surface }]} testID="branch-card">
        <T variant="mono" numberOfLines={1}>
          ⎇ {branch}
        </T>
        {items.length ? (
          <T variant="monoSm" color={theme.mut} style={styles.tabular}>
            {items.length} changed
          </T>
        ) : null}
      </View>
      {items.length === 0 ? (
        <EmptyState glyph="✓" title="No uncommitted changes" body={`${workspaceLabel(workspace)} matches its last commit on ${branch}.`} />
      ) : (
        items.map((item) => (
          <ChangeRow
            key={item.path}
            item={item}
            onPress={() => nav.navigate("Diff", { workspace_id: workspace.workspace_id, path: item.path })}
          />
        ))
      )}
      {truncated ? (
        <T variant="monoSm" color={theme.faint} style={styles.note}>
          More files changed than {hostName} lists here.
        </T>
      ) : null}
    </View>
  );
}

/** A changed file: its letter, its path, and `+n −n`. */
function ChangeRow({ item, onPress }: { item: ChangeItem; onPress(): void }) {
  const theme = useTheme();
  const color = item.letter === "A" || item.letter === "?" ? theme.done : item.letter === "D" ? theme.err : theme.mut;
  const slash = item.path.lastIndexOf("/");
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={`${item.path}, ${item.letter}`}
      onPress={onPress}
      style={({ pressed }) => [styles.row, pressed && { backgroundColor: theme.surface }]}
      testID={`change-${item.path}`}
    >
      <T variant="mono" color={color} style={styles.letter}>
        {item.letter}
      </T>
      <T variant="mono" numberOfLines={1} ellipsizeMode="head" style={styles.flex}>
        {slash >= 0 ? <T variant="mono" color={theme.mut}>{item.path.slice(0, slash + 1)}</T> : null}
        {item.path.slice(slash + 1)}
      </T>
      {item.binary || item.tooLarge ? (
        <T variant="monoSm" color={theme.faint}>
          {item.binary ? "binary" : "large"}
        </T>
      ) : (
        <T variant="monoSm" style={styles.tabular}>
          <T variant="monoSm" color={theme.done}>
            +{item.additions ?? 0}
          </T>{" "}
          <T variant="monoSm" color={theme.err}>
            −{item.deletions ?? 0}
          </T>
        </T>
      )}
    </Pressable>
  );
}

function Browse({
  workspace,
  path,
  onPath,
  loaded,
  hostName,
}: {
  workspace: WorkspaceInfo;
  path: string;
  onPath(path: string): void;
  loaded: Loaded<FileTreeNode[]>;
  hostName: string;
}) {
  const theme = useTheme();
  const nav = useRootNavigation();
  const trail = crumbs(workspaceLabel(workspace), path);
  const crumbsRef = useRef<ScrollView>(null);
  useEffect(() => {
    crumbsRef.current?.scrollToEnd({ animated: false });
  }, [path]);

  let body;
  if (loaded.error) {
    const p = fileProblem(loaded.error, hostName, "folder");
    body = (
      <EmptyState
        glyph="/"
        title={p.title}
        body={p.body}
        action={path ? <Button kind="secondary" label="Up one folder" onPress={() => onPath(parentPath(path))} /> : undefined}
      />
    );
  } else if (!loaded.data) body = <SkeletonRows />;
  else if (loaded.data.length === 0) {
    body = <EmptyState glyph="/" title="This folder is empty." body={`Files added on ${hostName} show up here.`} />;
  } else {
    body = folderEntries(loaded.data).map((node) => (
      <FileRow
        key={node.path}
        node={node}
        onPress={() =>
          node.kind === "dir" ? onPath(node.path) : nav.navigate("File", { workspace_id: workspace.workspace_id, path: node.path })
        }
      />
    ));
  }

  return (
    <View>
      <ScrollView
        ref={crumbsRef}
        horizontal
        showsHorizontalScrollIndicator={false}
        contentContainerStyle={styles.crumbs}
        testID="breadcrumb"
      >
        {trail.map((c, i) => {
          const last = i === trail.length - 1;
          return (
            <View key={c.path} style={styles.crumb}>
              {i > 0 ? (
                <T variant="mono" color={theme.faint}>
                  /
                </T>
              ) : null}
              <Pressable
                accessibilityRole="button"
                accessibilityLabel={last ? `${c.label}, this folder` : `Up to ${c.label}`}
                disabled={last}
                onPress={() => onPath(c.path)}
                hitSlop={{ top: 10, bottom: 10 }}
                style={styles.crumbButton}
                testID={`crumb-${c.path || "root"}`}
              >
                <T variant="mono" color={last ? theme.fg : theme.accent}>
                  {c.label}
                </T>
              </Pressable>
            </View>
          );
        })}
      </ScrollView>
      {body}
    </View>
  );
}

/** A folder or a file in Browse: folders end in `/` and open in place. */
function FileRow({ node, onPress }: { node: FileTreeNode; onPress(): void }) {
  const theme = useTheme();
  const dir = node.kind === "dir";
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={dir ? `Folder ${node.name}` : node.name}
      onPress={onPress}
      style={({ pressed }) => [styles.row, pressed && { backgroundColor: theme.surface }]}
      testID={`entry-${node.path}`}
    >
      <T variant="mono" color={dir ? theme.accent : theme.faint} style={styles.letter}>
        {dir ? "▸" : "·"}
      </T>
      <T variant="mono" numberOfLines={1} style={styles.flex}>
        {node.name}
        {dir ? <T variant="mono" color={theme.mut}>/</T> : null}
      </T>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  flex: { flex: 1 },
  tabular: { fontVariant: ["tabular-nums"] },
  segmented: { paddingHorizontal: space.lg, paddingBottom: space.sm },
  branch: {
    marginHorizontal: space.md,
    marginVertical: space.sm,
    borderRadius: radius.lg,
    paddingHorizontal: space.md,
    minHeight: 44,
    flexDirection: "row",
    alignItems: "center",
    justifyContent: "space-between",
    gap: space.sm,
  },
  row: { minHeight: 52, paddingHorizontal: space.lg, flexDirection: "row", alignItems: "center", gap: space.md },
  letter: { width: 14, textAlign: "center" },
  note: { paddingHorizontal: space.lg, paddingTop: space.sm },
  crumbs: { paddingHorizontal: space.lg, paddingVertical: space.sm, alignItems: "center" },
  crumb: { flexDirection: "row", alignItems: "center" },
  crumbButton: { minHeight: 32, justifyContent: "center", paddingHorizontal: space.xxs },
});
