// The screens pushed over the Files tab (DESIGN.md, Screen 6): the worktree's
// changed files, a file read-only with its line numbers, and a changed file's
// whole text with its changes marked, stepping through the others. Each carries `workspace · worktree`
// under its title; back goes up one level. What the host won't show as text
// (binary, too large, gone) says so.

import { useEffect, useMemo, useState, type ReactNode } from "react";
import { Pressable, ScrollView, StyleSheet, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import type { NativeStackNavigationProp, NativeStackScreenProps } from "@react-navigation/native-stack";
import * as Clipboard from "expo-clipboard";
import { isClientError, isMarkdown, type WorkspaceInfo } from "@zwiim/sdk";
import { useCurrentHost, useHostRead, useHostState, type Loaded } from "../app/hooks";
import type { RootStack } from "../app/navigation";
import {
  baseName,
  changeItems,
  changeLetter,
  changeName,
  changeTotals,
  fileProblem,
  notGitProblem,
  parentPath,
  previewLines,
  syntaxOf,
  type ChangeItem,
} from "../files/model";
import { loadViewed, TOO_LARGE } from "../files/viewer";
import { contextLabel, contextText, workspaceLabel } from "../host/workspace";
import { currentHost } from "../services";
import { Button, EmptyState, ScreenHeader, SkeletonRows, T } from "../ui/components";
import { CodeView, FileChangeView } from "../ui/code";
import { Markdown } from "../ui/markdown";
import { Check, ChevronRight, Copy, Eye, Icon } from "../ui/icons";
import { fonts, radius, space, useTheme, type Palette } from "../ui/tokens";
import { useHostName } from "./HostFrame";

export interface Changes {
  items: ChangeItem[];
  truncated: boolean;
}

/**
 * The worktree's uncommitted changes (`git.status`, `git.diff`), read again
 * when its files change on disk. Git is never asked about a plain folder.
 */
export function useChanges(workspace: WorkspaceInfo | null): Loaded<Changes> {
  const id = workspace?.workspace_id ?? "";
  const revision = useHostState().diffRevision[id] ?? 0;
  return useHostRead(workspace?.is_git_repo ? `${id}:${revision}` : null, async () => {
    const [status, diff] = await Promise.all([
      currentHost.request("git.status", { workspace_id: id, show_all_untracked: true }),
      currentHost.request("git.diff", { workspace_id: id }),
    ]);
    return { items: changeItems(status, diff), truncated: diff.truncated };
  });
}

/** A change's letter in its status color: added green, deleted red, the rest neutral. */
export function letterColor(letter: string, theme: Palette): string {
  if (letter === "A" || letter === "?") return theme.done;
  if (letter === "D") return theme.err;
  if (letter === "U") return theme.need;
  return theme.fg;
}

/** `+n −n` in mono, green and red. */
export function Counts({ additions, deletions, size }: { additions: number; deletions: number; size?: "sm" }) {
  const theme = useTheme();
  const variant = size === "sm" ? "monoSm" : "mono";
  return (
    <View style={styles.counts}>
      <T variant={variant} color={theme.done} style={styles.tabular}>
        +{additions}
      </T>
      <T variant={variant} color={theme.err} style={styles.tabular}>
        −{deletions}
      </T>
    </View>
  );
}

export function useWorkspaceInfo(workspace_id: string): { workspace: WorkspaceInfo | null; context: string; worktree: string } {
  const { workspaces } = useHostState();
  const workspace = workspaces?.find((w) => w.workspace_id === workspace_id) ?? null;
  const label = contextLabel(workspaces, workspace_id);
  return {
    workspace,
    context: contextText(label) ?? "",
    worktree: label?.worktree ?? (workspace ? workspaceLabel(workspace) : ""),
  };
}

/** "src/auth/ · zwiim · fix-auth": the file's folder, then where it lives. */
export function fileSubtitle(path: string, context: string): string {
  const folder = parentPath(path);
  return [folder ? `${folder}/` : null, context || null].filter(Boolean).join(" · ");
}

/** Back to where the user came from, or the Host shell if opened directly. */
export function useBack(navigation: { canGoBack(): boolean; goBack(): void; navigate(name: "Host"): void }) {
  return () => (navigation.canGoBack() ? navigation.goBack() : navigation.navigate("Host"));
}

/**
 * File ⇄ Diff of the same file without stacking them: back when the other is
 * right below, else pushed.
 */
function swapTo(navigation: NativeStackNavigationProp<RootStack>, to: "File" | "Diff", workspace_id: string, path: string) {
  const { routes } = navigation.getState();
  const below = routes[routes.length - 2];
  const params = below?.params as { workspace_id?: string; path?: string } | undefined;
  if (below?.name === to && params?.workspace_id === workspace_id && params.path === path) navigation.goBack();
  else navigation.push(to, { workspace_id, path });
}

/** The page under a pushed header, with the reconnecting dim; `surface` is the body's background. */
export function Page({ header, surface, children }: { header: ReactNode; surface?: "code"; children: ReactNode }) {
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  const { view } = useCurrentHost();
  const reconnecting = view?.status === "reconnecting";
  return (
    <View style={[styles.page, { backgroundColor: theme.bg, paddingTop: insets.top }]}>
      {header}
      <View
        style={[
          styles.page,
          { paddingBottom: insets.bottom },
          surface === "code" && { backgroundColor: theme.code },
          reconnecting && styles.dim,
        ]}
      >
        {children}
      </View>
    </View>
  );
}

export function ChangesScreen({ route, navigation }: NativeStackScreenProps<RootStack, "Changes">) {
  const theme = useTheme();
  const { workspace_id } = route.params;
  const hostName = useHostName();
  const { workspace, context, worktree } = useWorkspaceInfo(workspace_id);
  const back = useBack(navigation);
  const changes = useChanges(workspace);

  let body;
  if (workspace && !workspace.is_git_repo) {
    const p = notGitProblem(workspace);
    body = <EmptyState glyph="⎇" title={p.title} body={p.body} action={<Button kind="secondary" label="Browse files" onPress={back} />} />;
  } else if (changes.error) {
    const p = fileProblem(changes.error, hostName, "changes");
    body = <EmptyState glyph="⎇" title={p.title} body={p.body} action={<Button kind="secondary" label="Try again" onPress={changes.reload} />} />;
  } else if (!changes.data) body = <SkeletonRows />;
  else {
    const { items, truncated } = changes.data;
    const totals = changeTotals(items);
    body = (
      <>
        <View style={styles.branchLine} testID="changes-branch">
          <T variant="monoSm" color={theme.mut} numberOfLines={1} style={styles.flex}>
            ⎇ {workspace?.branch ?? "detached HEAD"}
          </T>
          {items.length ? <Counts size="sm" additions={totals.additions} deletions={totals.deletions} /> : null}
        </View>
        <ScrollView style={[styles.flex, { borderTopColor: theme.line }, styles.topLine]}>
          {items.length === 0 ? (
            <EmptyState
              glyph="✓"
              title="No uncommitted changes"
              body={`${worktree} matches its last commit.`}
              action={<Button kind="secondary" label="Browse files" onPress={back} />}
            />
          ) : (
            items.map((item) => (
              <ChangeRow key={item.path} item={item} onPress={() => navigation.push("Diff", { workspace_id, path: item.path })} />
            ))
          )}
          {truncated ? (
            <T variant="monoSm" color={theme.faint} style={styles.note}>
              More files changed than {hostName} lists here.
            </T>
          ) : null}
        </ScrollView>
      </>
    );
  }

  return <Page header={<ScreenHeader testID="changes" title="Changes" subtitle={context} onBack={back} />}>{body}</Page>;
}

/** A changed file: its letter in a tile, its name and folder, and `+n −n`. */
function ChangeRow({ item, onPress }: { item: ChangeItem; onPress(): void }) {
  const theme = useTheme();
  const name = baseName(item.path);
  const folder = parentPath(item.path);
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={`${item.path}, ${changeName(item.letter)}`}
      onPress={onPress}
      style={({ pressed }) => [styles.changeRow, { borderBottomColor: theme.line }, pressed && { backgroundColor: theme.surface }]}
      testID={`change-${item.path}`}
    >
      <View style={[styles.letterTile, { backgroundColor: theme.surface }]}>
        <T variant="mono" color={letterColor(item.letter, theme)} style={styles.letter}>
          {item.letter}
        </T>
      </View>
      <View style={styles.rowText}>
        <T variant="mono" numberOfLines={1} style={[styles.name, item.letter === "D" && styles.struck]}>
          {name}
        </T>
        <T variant="monoSm" color={theme.mut} numberOfLines={1} ellipsizeMode="head">
          {folder ? `${folder}/` : "./"} · {changeName(item.letter).toLowerCase()}
        </T>
      </View>
      {item.binary || item.tooLarge ? (
        <T variant="monoSm" color={theme.faint}>
          {item.binary ? "binary" : "large"}
        </T>
      ) : (
        <Counts size="sm" additions={item.additions ?? 0} deletions={item.deletions ?? 0} />
      )}
      <Icon icon={ChevronRight} color={theme.faint} size={18} />
    </Pressable>
  );
}

export function FileScreen({ route, navigation }: NativeStackScreenProps<RootStack, "File">) {
  const theme = useTheme();
  const { workspace_id, path } = route.params;
  const hostName = useHostName();
  const { workspace, context, worktree } = useWorkspaceInfo(workspace_id);
  const back = useBack(navigation);
  const [wrap, setWrap] = useState(true);
  const file = useHostRead(`${workspace_id}:${path}`, () => currentHost.request("files.read", { workspace_id, path }));
  const changes = useChanges(workspace);
  const change = changes.data?.items.find((c) => c.path === path) ?? null;
  const preview = useMemo(() => (file.data ? previewLines(file.data.content) : null), [file.data]);
  const name = baseName(path);

  let body;
  if (file.error) {
    const binary = isClientError(file.error, "invalid_input") && /binary/i.test(file.error instanceof Error ? file.error.message : "");
    const p = binary
      ? { title: `Can't preview ${name}`, body: `Binary file. Open it on ${hostName} to view it.` }
      : fileProblem(file.error, hostName, "file");
    body = <EmptyState glyph={binary ? "∅" : "✕"} title={p.title} body={p.body} action={binary ? undefined : <Button kind="secondary" label="Back" onPress={back} />} />;
  } else if (!file.data || !preview) body = <SkeletonRows />;
  else if (file.data.content === "") body = <EmptyState glyph="∅" title="This file is empty" body={`${name} has no content.`} />;
  else {
    const clipped = preview.total > preview.lines.length;
    body = (
      <CodeView
        lines={preview.lines}
        wrap={wrap}
        ext={syntaxOf(path)}
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
      surface={file.data?.content ? "code" : undefined}
      header={
        <ScreenHeader
          testID="file"
          title={name}
          subtitle={fileSubtitle(path, context)}
          onBack={back}
          trailing={
            file.data && file.data.content ? (
              <Pressable
                accessibilityRole="switch"
                accessibilityLabel="Wrap lines"
                accessibilityState={{ checked: wrap }}
                onPress={() => setWrap((w) => !w)}
                style={[styles.wrap, { backgroundColor: theme.surface }]}
                testID="file-wrap"
              >
                <T variant="monoSm" color={wrap ? theme.fg : theme.faint}>
                  wrap
                </T>
              </Pressable>
            ) : null
          }
        />
      }
    >
      {change ? (
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={`${changeName(change.letter)} on ${worktree}. Show diff`}
          onPress={() => swapTo(navigation, "Diff", workspace_id, path)}
          style={({ pressed }) => [
            styles.changed,
            { backgroundColor: pressed ? theme.raised : theme.surface, borderBottomColor: theme.line },
          ]}
          testID="file-changed"
        >
          <T variant="mono" color={letterColor(change.letter, theme)} style={styles.letter}>
            {change.letter}
          </T>
          <T numberOfLines={1} style={styles.flex}>
            {changeName(change.letter)} on {worktree}
          </T>
          {change.binary || change.tooLarge ? null : (
            <Counts size="sm" additions={change.additions ?? 0} deletions={change.deletions ?? 0} />
          )}
          <T variant="callout" color={theme.accent} style={styles.link}>
            Diff ›
          </T>
        </Pressable>
      ) : null}
      {body}
    </Page>
  );
}

export function DiffScreen({ route, navigation }: NativeStackScreenProps<RootStack, "Diff">) {
  const theme = useTheme();
  const { workspace_id, path } = route.params;
  const hostName = useHostName();
  const { workspace, context } = useWorkspaceInfo(workspace_id);
  const back = useBack(navigation);
  const revision = useHostState().diffRevision[workspace_id] ?? 0;
  const viewed = useHostRead(`${workspace_id}:${path}:${revision}`, () =>
    loadViewed(
      path,
      currentHost.request("git.diff", { workspace_id, file: path }),
      currentHost.request("files.read", { workspace_id, path }),
    ),
  );
  const changes = useChanges(workspace);
  const [opened, setOpened] = useState<Record<number, number>>({});
  const [preview, setPreview] = useState(false);
  useEffect(() => {
    setOpened({});
    setPreview(false);
  }, [path]);
  const v = viewed.data;
  const entry = v && v.kind !== "gone" ? v.file : null;
  const content = v?.kind === "lines" ? v.content : null;
  const list = changes.data?.items ?? [];
  const index = list.findIndex((c) => c.path === path);
  const letter = list[index]?.letter ?? (entry ? changeLetter(entry) : null);
  const canPreview = content !== null && isMarkdown(path);
  const previewing = preview && canPreview;
  const step = (by: number) => {
    const next = list[index + by];
    if (next) navigation.setParams({ path: next.path });
  };

  let body;
  if (viewed.error) {
    const p = fileProblem(viewed.error, hostName, "changes");
    body = <EmptyState glyph="✕" title={p.title} body={p.body} action={<Button kind="secondary" label="Back" onPress={back} />} />;
  } else if (!v) body = <SkeletonRows />;
  else if (v.kind === "binary") body = <EmptyState glyph="∅" title="Binary file" body="It can't be shown as text." />;
  else if (v.kind === "too_large") body = <EmptyState glyph="∅" title={TOO_LARGE} body={`Open it on ${hostName}.`} />;
  else if (v.kind === "gone") body = <EmptyState glyph="∅" title="This file is gone." body={`${baseName(path)} isn't on ${hostName} anymore.`} />;
  else if (previewing) {
    body = (
      <ScrollView contentContainerStyle={styles.preview} testID="diff-preview">
        <Markdown text={content} />
      </ScrollView>
    );
  } else {
    body = (
      <FileChangeView
        lines={v.lines}
        opened={opened}
        onReveal={(at, more) => setOpened((o) => ({ ...o, [at]: (o[at] ?? 0) + more }))}
        header={
          v.file ? null : (
            <T variant="callout" color={theme.mut} style={styles.unchanged}>
              No uncommitted changes — showing the file as it is now.
            </T>
          )
        }
      />
    );
  }

  return (
    <Page
      surface={v?.kind === "lines" && !previewing ? "code" : undefined}
      header={
        <ScreenHeader
          testID="diff"
          title={baseName(path)}
          subtitle={fileSubtitle(path, context)}
          onBack={back}
          trailing={
            letter && letter !== "D" ? (
              <Pressable
                accessibilityRole="button"
                onPress={() => swapTo(navigation, "File", workspace_id, path)}
                hitSlop={8}
                style={styles.headerLink}
                testID="diff-view-file"
              >
                <T variant="callout" color={theme.accent} style={styles.link}>
                  View file
                </T>
              </Pressable>
            ) : null
          }
        />
      }
    >
      {v && v.kind !== "gone" ? (
        <View style={[styles.diffStrip, { backgroundColor: theme.bg, borderBottomColor: theme.line }]}>
          {letter && entry ? (
            <View style={[styles.badge, { backgroundColor: theme.surface }]}>
              <T variant="monoSm" color={letterColor(letter, theme)}>
                {letter} · {changeName(letter)}
              </T>
            </View>
          ) : null}
          {entry && !entry.binary && !entry.too_large ? (
            <View testID="diff-counts">
              <Counts additions={entry.additions ?? 0} deletions={entry.deletions ?? 0} />
            </View>
          ) : null}
          <View style={styles.flex} />
          {canPreview ? (
            <StripButton
              icon={Eye}
              label="Preview"
              active={previewing}
              onPress={() => setPreview((p) => !p)}
              testID="diff-preview-toggle"
            />
          ) : null}
          <CopyButton content={content} />
        </View>
      ) : null}
      <View style={styles.flex}>{body}</View>
      {index >= 0 && list.length > 1 ? (
        <View style={[styles.stepper, { backgroundColor: theme.bg, borderTopColor: theme.line }]}>
          <Stepper label="‹ Prev" enabled={index > 0} onPress={() => step(-1)} testID="diff-prev" />
          <T variant="monoSm" color={theme.mut} style={styles.tabular} testID="diff-position">
            {index + 1} of {list.length}
          </T>
          <Stepper label="Next ›" enabled={index < list.length - 1} onPress={() => step(1)} align="right" testID="diff-next" />
        </View>
      ) : null}
    </Page>
  );
}

/** Copies the whole file as it is now; nothing to copy when it can't be read. */
function CopyButton({ content }: { content: string | null }) {
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const t = setTimeout(() => setCopied(false), 1500);
    return () => clearTimeout(t);
  }, [copied]);
  return (
    <StripButton
      icon={copied ? Check : Copy}
      label={copied ? "Copied" : "Copy"}
      disabled={content === null}
      onPress={() => {
        if (content === null) return;
        void Clipboard.setStringAsync(content).then(() => setCopied(true));
      }}
      testID="diff-copy"
    />
  );
}

function StripButton({
  icon,
  label,
  active,
  disabled,
  onPress,
  testID,
}: {
  icon: typeof Copy;
  label: string;
  active?: boolean;
  disabled?: boolean;
  onPress(): void;
  testID: string;
}) {
  const theme = useTheme();
  const color = active ? theme.fg : theme.mut;
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={label}
      accessibilityState={{ disabled: !!disabled, selected: !!active }}
      disabled={disabled}
      onPress={onPress}
      hitSlop={6}
      style={({ pressed }) => [
        styles.stripButton,
        { backgroundColor: active || pressed ? theme.surface : "transparent" },
        disabled && styles.dim,
      ]}
      testID={testID}
    >
      <Icon icon={icon} color={color} size={14} />
      <T variant="monoSm" color={color}>
        {label}
      </T>
    </Pressable>
  );
}

function Stepper({
  label,
  enabled,
  onPress,
  align,
  testID,
}: {
  label: string;
  enabled: boolean;
  onPress(): void;
  align?: "right";
  testID: string;
}) {
  const theme = useTheme();
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityState={{ disabled: !enabled }}
      disabled={!enabled}
      onPress={onPress}
      style={[styles.step, align === "right" && styles.stepRight]}
      testID={testID}
    >
      <T color={enabled ? theme.accent : theme.faint} style={styles.link}>
        {label}
      </T>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  page: { flex: 1 },
  flex: { flex: 1 },
  dim: { opacity: 0.6 },
  tabular: { fontVariant: ["tabular-nums"] },
  counts: { flexDirection: "row", gap: space.sm },
  topLine: { borderTopWidth: StyleSheet.hairlineWidth },
  branchLine: { minHeight: 44, flexDirection: "row", alignItems: "center", gap: space.md, paddingHorizontal: space.lg },
  changeRow: {
    minHeight: 60,
    flexDirection: "row",
    alignItems: "center",
    gap: space.md,
    paddingHorizontal: space.lg,
    paddingVertical: space.sm,
    borderBottomWidth: StyleSheet.hairlineWidth,
  },
  letterTile: { width: 24, height: 24, borderRadius: radius.sm, alignItems: "center", justifyContent: "center" },
  letter: { fontFamily: fonts.monoMedium },
  rowText: { flex: 1, gap: 3 },
  name: { fontSize: 14, lineHeight: 20 },
  struck: { textDecorationLine: "line-through" },
  note: { paddingHorizontal: space.lg, paddingTop: space.md },
  wrap: { height: 36, borderRadius: radius.sm, paddingHorizontal: space.md, justifyContent: "center" },
  clipped: { paddingTop: space.md, paddingLeft: space.lg },
  changed: {
    minHeight: 48,
    flexDirection: "row",
    alignItems: "center",
    gap: space.md,
    paddingHorizontal: space.lg,
    borderBottomWidth: StyleSheet.hairlineWidth,
  },
  link: { fontFamily: fonts.sansMedium, fontSize: 14 },
  headerLink: { height: 44, justifyContent: "center" },
  diffStrip: {
    minHeight: 44,
    flexDirection: "row",
    alignItems: "center",
    gap: space.md,
    paddingHorizontal: space.lg,
    borderBottomWidth: StyleSheet.hairlineWidth,
  },
  stripButton: {
    height: 28,
    flexDirection: "row",
    alignItems: "center",
    gap: space.xs,
    paddingHorizontal: space.sm,
    borderRadius: radius.sm,
  },
  preview: { padding: space.lg },
  unchanged: { paddingHorizontal: space.lg, paddingVertical: space.md },
  badge: { height: 22, borderRadius: radius.sm, paddingHorizontal: space.sm, justifyContent: "center" },
  stepper: {
    height: 56,
    flexDirection: "row",
    alignItems: "center",
    justifyContent: "space-between",
    paddingHorizontal: space.xs,
    borderTopWidth: StyleSheet.hairlineWidth,
  },
  step: { height: 44, minWidth: 88, paddingHorizontal: space.md, justifyContent: "center" },
  stepRight: { alignItems: "flex-end" },
});
