// A file and a diff, pushed over the Files tab (DESIGN.md, Screen 6): the
// file read-only with its line numbers, and a changed file's unified diff.
// What the host won't show as text (binary, too large, gone) says so.

import { useMemo, useState, type ReactNode } from "react";
import { Pressable, StyleSheet, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import type { NativeStackScreenProps } from "@react-navigation/native-stack";
import { useCurrentHost, useHostRead, useHostState } from "../app/hooks";
import type { RootStack } from "../app/navigation";
import { baseName, fileProblem, parentPath, parsePatch, previewLines } from "../files/model";
import { workspaceLabel } from "../sessions/model";
import { currentHost } from "../services";
import { Button, EmptyState, ScreenHeader, SkeletonRows, T } from "../ui/components";
import { CodeView, DiffView } from "../ui/code";
import { radius, space, useTheme } from "../ui/tokens";
import { useHostName } from "./HostFrame";

function useWorkspaceName(workspace_id: string): string {
  const { workspaces } = useHostState();
  const w = workspaces?.find((x) => x.workspace_id === workspace_id);
  return w ? workspaceLabel(w) : "";
}

/** Back to where the user came from, or the Host shell if opened directly. */
function useBack(navigation: { canGoBack(): boolean; goBack(): void; navigate(name: "Host"): void }) {
  return () => (navigation.canGoBack() ? navigation.goBack() : navigation.navigate("Host"));
}

/** The `code` surface under a pushed header, with the reconnecting dim. */
function Page({ header, children }: { header: ReactNode; children: ReactNode }) {
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  const { view } = useCurrentHost();
  const reconnecting = view?.status === "reconnecting";
  return (
    <View style={[styles.page, { backgroundColor: theme.bg, paddingTop: insets.top }]}>
      {header}
      <View style={[styles.page, { paddingBottom: insets.bottom }, reconnecting && styles.dim]}>{children}</View>
    </View>
  );
}

export function FileScreen({ route, navigation }: NativeStackScreenProps<RootStack, "File">) {
  const theme = useTheme();
  const { workspace_id, path } = route.params;
  const hostName = useHostName();
  const workspace = useWorkspaceName(workspace_id);
  const back = useBack(navigation);
  const [wrap, setWrap] = useState(false);
  const file = useHostRead(`${workspace_id}:${path}`, () => currentHost.request("files.read", { workspace_id, path }));
  const preview = useMemo(() => (file.data ? previewLines(file.data.content) : null), [file.data]);

  let body;
  if (file.error) {
    const p = fileProblem(file.error, hostName, "file");
    body = <EmptyState glyph="✕" title={p.title} body={p.body} action={<Button kind="secondary" label="Back" onPress={back} />} />;
  } else if (!file.data || !preview) body = <SkeletonRows />;
  else if (file.data.content === "") body = <EmptyState glyph="∅" title="This file is empty" body={`${baseName(path)} has no content.`} />;
  else {
    const clipped = preview.total > preview.lines.length;
    body = (
      <CodeView
        lines={preview.lines}
        wrap={wrap}
        footer={
          clipped ? (
            <T variant="monoSm" color={theme.mut} style={styles.clipped}>
              Showing the first {preview.lines.length.toLocaleString()} of {preview.total.toLocaleString()} lines.
            </T>
          ) : null
        }
      />
    );
  }

  return (
    <Page
      header={
        <ScreenHeader
          testID="file"
          title={baseName(path)}
          subtitle={[workspace, parentPath(path)].filter(Boolean).join(" · ")}
          onBack={back}
          trailing={
            file.data && file.data.content ? (
              <Pressable
                accessibilityRole="switch"
                accessibilityLabel="Wrap lines"
                accessibilityState={{ checked: wrap }}
                onPress={() => setWrap((w) => !w)}
                hitSlop={6}
                style={[styles.toggle, { backgroundColor: wrap ? theme.raised : theme.surface }]}
                testID="file-wrap"
              >
                <T variant="monoSm" color={wrap ? theme.accent : theme.mut}>
                  ↩ wrap
                </T>
              </Pressable>
            ) : null
          }
        />
      }
    >
      {body}
    </Page>
  );
}

export function DiffScreen({ route, navigation }: NativeStackScreenProps<RootStack, "Diff">) {
  const theme = useTheme();
  const { workspace_id, path } = route.params;
  const hostName = useHostName();
  const workspace = useWorkspaceName(workspace_id);
  const back = useBack(navigation);
  const revision = useHostState().diffRevision[workspace_id] ?? 0;
  const diff = useHostRead(`${workspace_id}:${path}:${revision}`, () =>
    currentHost.request("git.diff", { workspace_id, file: path }),
  );
  const hunks = useMemo(() => (diff.data?.patch ? parsePatch(diff.data.patch) : []), [diff.data]);
  const entry = diff.data?.files.find((f) => f.path === path) ?? null;

  let body;
  if (diff.error) {
    const p = fileProblem(diff.error, hostName, "changes");
    body = <EmptyState glyph="✕" title={p.title} body={p.body} action={<Button kind="secondary" label="Back" onPress={back} />} />;
  } else if (!diff.data) body = <SkeletonRows />;
  else if (entry?.binary) body = <EmptyState glyph="✕" title="Binary file" body="Its changes can't be shown as text." />;
  else if (entry?.too_large || (entry && diff.data.patch === null)) {
    body = <EmptyState glyph="✕" title="Too large to show" body={`Open the diff on ${hostName}.`} />;
  } else if (!entry || hunks.length === 0) {
    body = <EmptyState glyph="✓" title="No changes" body={`${baseName(path)} matches its last commit.`} />;
  } else body = <DiffView hunks={hunks} />;

  const counts =
    entry && !entry.binary && !entry.too_large ? `+${entry.additions ?? 0} −${entry.deletions ?? 0}` : entry?.binary ? "binary" : null;
  return (
    <Page
      header={
        <ScreenHeader
          testID="diff"
          title={baseName(path)}
          subtitle={[workspace, parentPath(path)].filter(Boolean).join(" · ")}
          onBack={back}
          trailing={
            counts ? (
              <T variant="monoSm" color={theme.mut} style={styles.tabular} testID="diff-counts">
                {counts}
              </T>
            ) : null
          }
        />
      }
    >
      {body}
    </Page>
  );
}

const styles = StyleSheet.create({
  page: { flex: 1 },
  dim: { opacity: 0.6 },
  tabular: { fontVariant: ["tabular-nums"] },
  toggle: { height: 32, borderRadius: radius.sm, paddingHorizontal: space.sm, justifyContent: "center" },
  clipped: { paddingTop: space.md },
});
