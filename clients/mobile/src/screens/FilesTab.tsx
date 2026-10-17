// The Files tab (DESIGN.md, Screen 6): the filesystem is the page. The
// context chips set the worktree and everything under them belongs to it: one
// Changes row with the count and `+n −n` (it opens the changed files), the
// worktree's folders one at a time with a path bar and a file search
// (`workspace.tree`), and the Terminals dock above the tab bar. Changed files
// carry their letter in the tree. A plain folder (not a Git repository) says
// so in place of the Changes row, unasked.

import { useCallback, useEffect, useRef, useState } from "react";
import { BackHandler, FlatList, Pressable, ScrollView, StyleSheet, TextInput, View } from "react-native";
import { useFocusEffect } from "@react-navigation/native";
import type { FileTreeNode, WorkspaceInfo } from "@zwiim/sdk";
import { useHostRead, useHostState, useWorkspace, type Loaded } from "../app/hooks";
import { useRootNavigation } from "../app/navigation";
import {
  browseRows,
  changedFilesText,
  changeTotals,
  crumbs,
  fileProblem,
  findFiles,
  parentPath,
  type BrowseRow,
  type ChangeItem,
  type FoundFile,
} from "../files/model";
import { contextLabel, contextText, workspaceLabel } from "../host/workspace";
import { openTarget } from "../preview/model";
import { currentHost } from "../services";
import { Button, EmptyState, SkeletonRows, T } from "../ui/components";
import { ChevronLeft, ChevronRight, Icon, Search } from "../ui/icons";
import { fonts, radius, space, type, useTheme } from "../ui/tokens";
import { Counts, letterColor, useChanges, type Changes } from "./FilesScreens";
import { HostFrame, useHostName } from "./HostFrame";
import { TerminalsDock } from "./TerminalScreens";
import { ContextBar, NewWorkspaceButton } from "./workspaces";

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
  return (
    <HostFrame fill>
      {workspace ? (
        <>
          <ContextBar />
          <Files key={workspace.workspace_id} workspace={workspace} />
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

function Files({ workspace }: { workspace: WorkspaceInfo }) {
  const theme = useTheme();
  const nav = useRootNavigation();
  const hostName = useHostName();
  const id = workspace.workspace_id;
  const { workspaces, diffRevision } = useHostState();
  const revision = diffRevision[id] ?? 0;
  const label = contextLabel(workspaces, id);
  const where = contextText(label) ?? workspaceLabel(workspace);
  const scope = label?.worktree ?? workspaceLabel(workspace);
  const [path, setPath] = useState("");
  const [searching, setSearching] = useState(false);
  const [query, setQuery] = useState("");

  const changes = useChanges(workspace);
  // Two levels: each folder listed knows how many entries it holds.
  const tree = useHostRead(`${id}:${path}`, () =>
    currentHost.request("workspace.tree", path ? { workspace_id: id, path, max_depth: 2 } : { workspace_id: id, max_depth: 2 }),
  );
  // The whole worktree, read once a search starts (the host skips build and dependency folders).
  const all = useHostRead(searching ? `${id}:all:${revision}` : null, () => currentHost.request("workspace.tree", { workspace_id: id }));
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

  const closeSearch = () => {
    setSearching(false);
    setQuery("");
  };

  // Android back ends a search, then climbs out of a folder, before leaving the tab.
  useFocusEffect(
    useCallback(() => {
      if (!searching && !path) return;
      const sub = BackHandler.addEventListener("hardwareBackPress", () => {
        if (searching) closeSearch();
        else setPath(parentPath(path));
        return true;
      });
      return () => sub.remove();
    }, [searching, path]),
  );

  const items = changes.data?.items ?? [];
  const openFile = (file: string) => nav.navigate(openTarget(file), { workspace_id: id, path: file });

  return (
    <View style={styles.flex}>
      <ChangesLine workspace={workspace} loaded={changes} scope={scope} hostName={hostName} />
      <View style={[styles.pathBar, { borderColor: theme.line }]}>
        {searching ? (
          <SearchField query={query} onQuery={setQuery} scope={scope} onCancel={closeSearch} />
        ) : (
          <PathTrail
            workspace={workspaceLabel(workspace)}
            path={path}
            onPath={setPath}
            onSearch={() => setSearching(true)}
          />
        )}
      </View>
      <View style={styles.flex}>
        {searching ? (
          <SearchResults query={query} loaded={all} changes={items} where={where} hostName={hostName} onOpen={openFile} />
        ) : (
          <Folder
            path={path}
            loaded={tree}
            changes={items}
            where={where}
            hostName={hostName}
            onPath={setPath}
            onOpen={openFile}
          />
        )}
      </View>
      <TerminalsDock workspace={workspace} />
    </View>
  );
}

/**
 * The worktree's changes in one line above the tree: `4 changed files +12 −4 ›`
 * (opens them), clean, or not a Git repository.
 */
function ChangesLine({
  workspace,
  loaded,
  scope,
  hostName,
}: {
  workspace: WorkspaceInfo;
  loaded: Loaded<Changes>;
  scope: string;
  hostName: string;
}) {
  const theme = useTheme();
  const nav = useRootNavigation();
  if (!workspace.is_git_repo) {
    return (
      <View style={styles.note} testID="changes-not-git">
        <T variant="monoSm" color={theme.faint}>
          ⎇
        </T>
        <T variant="monoSm" color={theme.mut} numberOfLines={1}>
          Not a Git repository · plain folder
        </T>
      </View>
    );
  }
  if (loaded.error) {
    return (
      <Pressable accessibilityRole="button" onPress={loaded.reload} style={styles.note} testID="changes-error">
        <T variant="monoSm" color={theme.err}>
          ✕
        </T>
        <T variant="monoSm" color={theme.mut} numberOfLines={1} style={styles.flex}>
          {fileProblem(loaded.error, hostName, "changes").title}
        </T>
        <T variant="monoSm" color={theme.accent}>
          Retry
        </T>
      </Pressable>
    );
  }
  if (!loaded.data) return <View style={styles.note} />;
  const { items, truncated } = loaded.data;
  if (items.length === 0) {
    return (
      <View style={styles.note} testID="changes-clean">
        <T variant="monoSm" color={theme.done}>
          ✓
        </T>
        <T variant="monoSm" color={theme.mut} numberOfLines={1}>
          No changes on {scope}
        </T>
      </View>
    );
  }
  const totals = changeTotals(items);
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={`${changedFilesText(items.length)}, ${totals.additions} added, ${totals.deletions} removed`}
      onPress={() => nav.navigate("Changes", { workspace_id: workspace.workspace_id })}
      style={({ pressed }) => [styles.changesRow, { backgroundColor: pressed ? theme.raised : theme.surface }]}
      testID="changes-row"
    >
      <T style={styles.changesTitle}>
        {changedFilesText(items.length)}
        {truncated ? "+" : ""}
      </T>
      <Counts additions={totals.additions} deletions={totals.deletions} />
      <Icon icon={ChevronRight} color={theme.faint} size={18} />
    </Pressable>
  );
}

/** `‹  zwiim / src / auth   ⌕`: up one folder, the path (tap a segment to jump up), and search. */
function PathTrail({
  workspace,
  path,
  onPath,
  onSearch,
}: {
  workspace: string;
  path: string;
  onPath(path: string): void;
  onSearch(): void;
}) {
  const theme = useTheme();
  const trail = crumbs(workspace, path);
  const ref = useRef<ScrollView>(null);
  useEffect(() => {
    ref.current?.scrollToEnd({ animated: false });
  }, [path]);
  return (
    <>
      {path ? (
        <Pressable
          accessibilityRole="button"
          accessibilityLabel="Up one folder"
          onPress={() => onPath(parentPath(path))}
          style={styles.iconButton}
          testID="folder-up"
        >
          <Icon icon={ChevronLeft} color={theme.fg} size={22} />
        </Pressable>
      ) : null}
      <ScrollView
        ref={ref}
        horizontal
        showsHorizontalScrollIndicator={false}
        style={styles.flex}
        contentContainerStyle={[styles.crumbs, !path && styles.crumbsTop]}
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
                style={styles.crumbButton}
                testID={`crumb-${c.path || "root"}`}
              >
                <T variant="mono" color={last ? theme.fg : theme.mut}>
                  {c.label}
                </T>
              </Pressable>
            </View>
          );
        })}
      </ScrollView>
      <Pressable accessibilityRole="button" accessibilityLabel="Find a file" onPress={onSearch} style={styles.iconButton} testID="files-search">
        <Icon icon={Search} color={theme.mut} size={18} />
      </Pressable>
    </>
  );
}

function SearchField({ query, onQuery, scope, onCancel }: { query: string; onQuery(q: string): void; scope: string; onCancel(): void }) {
  const theme = useTheme();
  return (
    <>
      <View style={[styles.searchBox, { backgroundColor: theme.surface }]}>
        <TextInput
          autoFocus
          value={query}
          onChangeText={onQuery}
          placeholder={`Find a file in ${scope}`}
          placeholderTextColor={theme.faint}
          autoCapitalize="none"
          autoCorrect={false}
          spellCheck={false}
          returnKeyType="search"
          style={[type.mono, styles.searchInput, { color: theme.fg }]}
          accessibilityLabel="Find a file"
          testID="files-search-input"
        />
      </View>
      <Pressable accessibilityRole="button" onPress={onCancel} style={styles.cancel} testID="files-search-cancel">
        <T color={theme.accent} style={styles.cancelText}>
          Cancel
        </T>
      </Pressable>
    </>
  );
}

/** One folder of the worktree, folders first; files open read-only. */
function Folder({
  path,
  loaded,
  changes,
  where,
  hostName,
  onPath,
  onOpen,
}: {
  path: string;
  loaded: Loaded<FileTreeNode[]>;
  changes: readonly ChangeItem[];
  where: string;
  hostName: string;
  onPath(path: string): void;
  onOpen(path: string): void;
}) {
  const up = path ? <Button kind="secondary" label="Up one folder" onPress={() => onPath(parentPath(path))} /> : undefined;
  if (loaded.error) {
    const p = fileProblem(loaded.error, hostName, "folder");
    return <EmptyState glyph="/" title={p.title} body={p.body} action={up} />;
  }
  if (!loaded.data) return <SkeletonRows />;
  const rows = browseRows(loaded.data, changes);
  if (rows.length === 0) {
    const folder = path.split("/").at(-1) || where.split(" · ")[0];
    return <EmptyState glyph="∅" title="This folder is empty" body={`${folder}/ has nothing in it on ${where}.`} action={up} />;
  }
  return (
    <FlatList
      data={rows}
      keyExtractor={(r) => r.node.path}
      renderItem={({ item }) => (
        <EntryRow row={item} onPress={() => (item.node.kind === "dir" ? onPath(item.node.path) : onOpen(item.node.path))} />
      )}
    />
  );
}

function SearchResults({
  query,
  loaded,
  changes,
  where,
  hostName,
  onOpen,
}: {
  query: string;
  loaded: Loaded<FileTreeNode[]>;
  changes: readonly ChangeItem[];
  where: string;
  hostName: string;
  onOpen(path: string): void;
}) {
  const theme = useTheme();
  if (!query.trim()) {
    return (
      <T variant="monoSm" color={theme.mut} style={styles.hint}>
        Type part of a file name
      </T>
    );
  }
  if (loaded.error) {
    const p = fileProblem(loaded.error, hostName, "folder");
    return <EmptyState glyph="/" title={p.title} body={p.body} action={<Button kind="secondary" label="Try again" onPress={loaded.reload} />} />;
  }
  if (!loaded.data) return <SkeletonRows />;
  const found = findFiles(loaded.data, query);
  if (found.length === 0) {
    return <EmptyState glyph="∅" title={`No files named “${query.trim()}”`} body={`Searched every folder in ${where}.`} />;
  }
  const letters = new Map(changes.map((c) => [c.path, c.letter]));
  return (
    <FlatList
      data={found}
      keyExtractor={(f) => f.path}
      keyboardShouldPersistTaps="handled"
      renderItem={({ item }) => <FoundRow file={item} letter={letters.get(item.path) ?? null} onPress={() => onOpen(item.path)} />}
    />
  );
}

/** A folder or a file: folders end in `/`, say what they hold, and open in place. */
function EntryRow({ row, onPress }: { row: BrowseRow; onPress(): void }) {
  const theme = useTheme();
  const { node, meta, letter, changed } = row;
  const dir = node.kind === "dir";
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={[dir ? `Folder ${node.name}` : node.name, letter ? `changed, ${letter}` : changed ? "has changes" : null]
        .filter(Boolean)
        .join(", ")}
      onPress={onPress}
      style={({ pressed }) => [styles.row, { borderBottomColor: theme.line }, pressed && { backgroundColor: theme.surface }]}
      testID={`entry-${node.path}`}
    >
      <T variant="mono" numberOfLines={1} style={styles.flex}>
        {node.name}
        {dir ? <T variant="mono" color={theme.faint}>/</T> : null}
      </T>
      {changed ? <View style={[styles.changedDot, { backgroundColor: theme.fg }]} testID={`entry-changed-${node.path}`} /> : null}
      {letter ? (
        <T variant="mono" color={letterColor(letter, theme)} style={styles.letter}>
          {letter}
        </T>
      ) : null}
      {meta ? (
        <T variant="monoSm" color={theme.mut} style={styles.tabular}>
          {meta}
        </T>
      ) : null}
      {dir ? <Icon icon={ChevronRight} color={theme.faint} size={18} /> : null}
    </Pressable>
  );
}

/** A file a search found, with the folder it is in. */
function FoundRow({ file, letter, onPress }: { file: FoundFile; letter: string | null; onPress(): void }) {
  const theme = useTheme();
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={`${file.name}, in ${file.folder}`}
      onPress={onPress}
      style={({ pressed }) => [styles.row, { borderBottomColor: theme.line }, pressed && { backgroundColor: theme.surface }]}
      testID={`found-${file.path}`}
    >
      <View style={styles.found}>
        <T variant="mono" numberOfLines={1}>
          {file.name}
        </T>
        <T variant="monoSm" color={theme.mut} numberOfLines={1} ellipsizeMode="head">
          {file.folder}
        </T>
      </View>
      {letter ? (
        <T variant="mono" color={letterColor(letter, theme)} style={styles.letter}>
          {letter}
        </T>
      ) : null}
    </Pressable>
  );
}

const styles = StyleSheet.create({
  flex: { flex: 1 },
  tabular: { fontVariant: ["tabular-nums"] },
  note: { minHeight: 36, flexDirection: "row", alignItems: "center", gap: space.sm, marginHorizontal: space.lg, marginTop: space.xs, marginBottom: space.sm },
  changesRow: {
    minHeight: 52,
    flexDirection: "row",
    alignItems: "center",
    gap: space.md,
    marginHorizontal: space.md,
    marginTop: space.xs,
    marginBottom: space.sm,
    paddingHorizontal: space.md,
    borderRadius: radius.md,
  },
  changesTitle: { flex: 1, fontFamily: fonts.sansMedium },
  pathBar: {
    height: 48,
    flexDirection: "row",
    alignItems: "center",
    paddingHorizontal: space.xs,
    borderTopWidth: StyleSheet.hairlineWidth,
    borderBottomWidth: StyleSheet.hairlineWidth,
  },
  iconButton: { width: 44, height: 44, alignItems: "center", justifyContent: "center" },
  crumbs: { alignItems: "center", paddingRight: space.sm },
  crumbsTop: { paddingLeft: space.md },
  crumb: { flexDirection: "row", alignItems: "center" },
  crumbButton: { height: 40, justifyContent: "center", paddingHorizontal: 3 },
  searchBox: { flex: 1, height: 36, marginLeft: space.sm, borderRadius: radius.md, paddingHorizontal: space.sm, justifyContent: "center" },
  searchInput: { padding: 0 },
  cancel: { height: 44, paddingHorizontal: space.md, justifyContent: "center" },
  cancelText: { fontFamily: fonts.sansMedium },
  hint: { padding: space.lg },
  row: {
    minHeight: 52,
    flexDirection: "row",
    alignItems: "center",
    gap: space.sm,
    paddingHorizontal: space.lg,
    borderBottomWidth: StyleSheet.hairlineWidth,
  },
  found: { flex: 1, gap: space.xxs, paddingVertical: space.sm },
  changedDot: { width: 6, height: 6, borderRadius: radius.full, opacity: 0.85 },
  letter: { width: 18, textAlign: "center", fontFamily: fonts.monoMedium },
});
