// Browse Mac (DESIGN.md, New workspace): the host user's home folder, one
// level at a time (`workspace.browse`), to add a folder as a workspace
// (`workspace.add { home_path }`). New folder makes one folder where you are
// (`workspace.create_folder`) and opens it, ready to add. Folders are named by
// segments under `~`; the host resolves them and never lets the phone above
// home. Leaving without Add registers nothing. Opened from New session, Add
// returns there with the new workspace as where the session starts, and
// Files stays where it was.

import { useCallback, useState } from "react";
import { BackHandler, Pressable, ScrollView, StyleSheet, TextInput, View } from "react-native";
import { useFocusEffect } from "@react-navigation/native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import type { NativeStackScreenProps } from "@react-navigation/native-stack";
import { isClientError, type BrowseEntry } from "@zwiim/sdk";
import { useCurrentHost, useHostRead } from "../app/hooks";
import type { RootStack } from "../app/navigation";
import {
  addLabel,
  addProblem,
  browseProblem,
  canAdd,
  canCreateFolder,
  createFolderProblem,
  folderNameProblem,
  homeText,
  upPath,
} from "../files/browse";
import { currentHost } from "../services";
import { Banner, Button, EmptyState, ScreenHeader, Sheet, SkeletonRows, T } from "../ui/components";
import { radius, space, type, useTheme } from "../ui/tokens";
import { useHostName } from "./HostFrame";

export function BrowseMacScreen({ navigation, route }: NativeStackScreenProps<RootStack, "BrowseMac">) {
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  const hostName = useHostName();
  const { view } = useCurrentHost();
  const live = view?.status === "live";
  const [path, setPath] = useState<string[]>([]);
  const [adding, setAdding] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [naming, setNaming] = useState(false);
  const listing = useHostRead(homeText(path), () => currentHost.request("workspace.browse", { path }));
  const close = () => (navigation.canGoBack() ? navigation.goBack() : navigation.navigate("Host", { screen: "Files" }));
  const open = (next: string[]) => {
    setError(null);
    setPath(next);
  };

  // Android back climbs one folder before leaving.
  useFocusEffect(
    useCallback(() => {
      if (!path.length) return;
      const sub = BackHandler.addEventListener("hardwareBackPress", () => {
        setError(null);
        setPath(upPath(path));
        return true;
      });
      return () => sub.remove();
    }, [path]),
  );

  const add = async () => {
    setAdding(true);
    setError(null);
    try {
      // Back to the screen underneath, not a second one on top of Browse Mac.
      // For New session it is where the session starts; Files keeps its own selection.
      if (route.params?.then === "NewSession") {
        const workspace = await currentHost.addWorkspaceAt(path, { choose: false });
        navigation.popTo("NewSession", { workspace_id: workspace.workspace_id });
      } else {
        await currentHost.addWorkspaceAt(path);
        navigation.popTo("Host", { screen: "Files" });
      }
    } catch (e) {
      setError(addProblem(e, path, hostName));
    } finally {
      setAdding(false);
    }
  };

  let body;
  if (listing.error) {
    const p = browseProblem(listing.error, path, hostName);
    body = (
      <EmptyState
        glyph="/"
        title={p.title}
        body={p.body}
        action={
          path.length ? (
            <Button kind="secondary" label="Up one folder" onPress={() => open(upPath(path))} />
          ) : (
            <Button kind="secondary" label="Try again" onPress={listing.reload} />
          )
        }
      />
    );
  } else if (!listing.data) body = <SkeletonRows />;
  else {
    const { entries, truncated } = listing.data;
    body = (
      <>
        {path.length ? <UpRow onPress={() => open(upPath(path))} /> : null}
        <NewFolderRow disabled={!live} onPress={() => setNaming(true)} />
        {entries.length === 0 ? (
          <T color={theme.mut} style={styles.note}>
            No folders in here.
          </T>
        ) : (
          entries.map((entry) => <FolderRow key={entry.name} entry={entry} onPress={() => open([...path, entry.name])} />)
        )}
        {truncated ? (
          <T variant="monoSm" color={theme.faint} style={styles.note}>
            Showing the first 1,000 folders.
          </T>
        ) : null}
      </>
    );
  }

  return (
    <View style={[styles.page, { backgroundColor: theme.bg, paddingTop: insets.top }]}>
      <ScreenHeader testID="browse" close title="Browse Mac" subtitle={homeText(path)} onBack={close} />
      <ScrollView style={[styles.page, !live && styles.dim]} contentContainerStyle={styles.list}>
        {body}
      </ScrollView>
      {error ? <Banner text={error} /> : null}
      {canAdd(path) && !listing.error ? (
        <View style={[styles.bar, { borderTopColor: theme.line, paddingBottom: Math.max(insets.bottom, space.sm) }]}>
          <Button tall label={addLabel(path)} loading={adding} disabled={!live} onPress={() => void add()} testID="browse-add" />
        </View>
      ) : null}
      <NewFolderSheet
        visible={naming}
        path={path}
        live={live}
        hostName={hostName}
        onClose={() => setNaming(false)}
        onCreated={(name) => {
          setNaming(false);
          open([...path, name]);
        }}
        onUnsure={listing.reload}
      />
    </View>
  );
}

function NewFolderRow({ disabled, onPress }: { disabled: boolean; onPress(): void }) {
  const theme = useTheme();
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel="New folder"
      accessibilityState={{ disabled }}
      disabled={disabled}
      onPress={onPress}
      style={({ pressed }) => [styles.row, pressed && { backgroundColor: theme.surface }, disabled && styles.dim]}
      testID="browse-new-folder"
    >
      <T variant="mono" color={theme.accent} style={styles.glyph}>
        +
      </T>
      <T color={theme.accent}>New folder</T>
    </Pressable>
  );
}

/** Names one new folder inside `path`; the host makes it, then it opens. */
function NewFolderSheet({
  visible,
  path,
  live,
  hostName,
  onClose,
  onCreated,
  onUnsure,
}: {
  visible: boolean;
  path: string[];
  live: boolean;
  hostName: string;
  onClose(): void;
  onCreated(name: string): void;
  onUnsure(): void;
}) {
  const theme = useTheme();
  const [name, setName] = useState("");
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const problem = error ?? folderNameProblem(name);
  const reset = () => {
    setName("");
    setError(null);
  };
  const create = async () => {
    if (!canCreateFolder(name) || creating) return;
    const folder = name.trim();
    setCreating(true);
    setError(null);
    try {
      await currentHost.request("workspace.create_folder", { path, name: folder });
      reset();
      onCreated(folder);
    } catch (e) {
      setError(createFolderProblem(e, path, folder, hostName));
      if (isClientError(e, "unknown_outcome")) onUnsure();
    } finally {
      setCreating(false);
    }
  };
  return (
    <Sheet
      visible={visible}
      onClose={() => {
        if (creating) return;
        reset();
        onClose();
      }}
    >
      <View style={styles.sheetHead}>
        <T variant="title">New folder</T>
        <T variant="monoSm" color={theme.mut}>
          in {homeText(path)}
        </T>
      </View>
      <TextInput
        value={name}
        onChangeText={(t) => {
          setName(t);
          setError(null);
        }}
        placeholder="Folder name"
        placeholderTextColor={theme.faint}
        autoFocus
        autoCapitalize="none"
        autoCorrect={false}
        returnKeyType="done"
        onSubmitEditing={() => void create()}
        style={[type.body, styles.input, { color: theme.fg, backgroundColor: theme.surface }]}
        accessibilityLabel="Folder name"
        testID="new-folder-name"
      />
      {problem ? (
        <T color={theme.err} testID="new-folder-error">
          {problem}
        </T>
      ) : null}
      <Button
        tall
        label="Create folder"
        loading={creating}
        disabled={!live || !canCreateFolder(name)}
        onPress={() => void create()}
        testID="new-folder-create"
      />
    </Sheet>
  );
}

function UpRow({ onPress }: { onPress(): void }) {
  const theme = useTheme();
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel="Up one folder"
      onPress={onPress}
      style={({ pressed }) => [styles.row, pressed && { backgroundColor: theme.surface }]}
      testID="browse-up"
    >
      <T variant="mono" color={theme.accent} style={styles.glyph}>
        ‹
      </T>
      <T variant="mono" color={theme.mut}>
        ..
      </T>
    </Pressable>
  );
}

/** A folder: its name and Git / Workspace tags; one the host can't read is dimmed and closed. */
function FolderRow({ entry, onPress }: { entry: BrowseEntry; onPress(): void }) {
  const theme = useTheme();
  const tags = [entry.is_git_repo ? "Git" : null, entry.is_workspace ? "Workspace" : null].filter((t): t is string => t !== null);
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={[`Folder ${entry.name}`, ...tags, entry.is_readable ? null : "can't be opened"].filter(Boolean).join(", ")}
      accessibilityState={{ disabled: !entry.is_readable }}
      disabled={!entry.is_readable}
      onPress={onPress}
      style={({ pressed }) => [styles.row, pressed && { backgroundColor: theme.surface }, !entry.is_readable && styles.dim]}
      testID={`browse-${entry.name}`}
    >
      <T variant="mono" color={theme.accent} style={styles.glyph}>
        ▸
      </T>
      <T variant="mono" numberOfLines={1} style={styles.flex}>
        {entry.name}
        <T variant="mono" color={theme.mut}>
          /
        </T>
      </T>
      {tags.map((tag) => (
        <View key={tag} style={[styles.tag, { backgroundColor: theme.surface }]}>
          <T variant="monoSm" color={theme.mut}>
            {tag}
          </T>
        </View>
      ))}
    </Pressable>
  );
}

const styles = StyleSheet.create({
  page: { flex: 1 },
  flex: { flex: 1 },
  dim: { opacity: 0.5 },
  list: { paddingVertical: space.sm },
  row: { minHeight: 52, paddingHorizontal: space.lg, flexDirection: "row", alignItems: "center", gap: space.md },
  glyph: { width: 14, textAlign: "center" },
  tag: { borderRadius: radius.sm, paddingHorizontal: space.sm, paddingVertical: space.xxs },
  note: { paddingHorizontal: space.lg, paddingTop: space.sm },
  sheetHead: { gap: space.xxs },
  input: { height: 44, borderRadius: radius.md, paddingHorizontal: space.md },
  bar: { borderTopWidth: StyleSheet.hairlineWidth, paddingHorizontal: space.lg, paddingTop: space.sm },
});
