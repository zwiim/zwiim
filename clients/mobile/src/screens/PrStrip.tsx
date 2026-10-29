// The PR strip (DESIGN.md, Screen 5): under the session strip, the worktree
// branch's pull request as the Host follows it on GitHub. One line: `#12`,
// its status and checks; it opens the PR sheet (title, base ← head, each
// check, Merge). Merge turns the sheet into its confirm, and the Host merges
// at the head shown there or refuses. Once merged, Continue moves the worktree
// to a new branch from the PR's base, and the strip goes. The phone never asks
// GitHub. A branch that conflicts with its base says so on the strip (with or
// without a PR), and the sheet hands the conflicts to the session's agent.

import { useEffect, useState } from "react";
import { Linking, Pressable, ScrollView, StyleSheet, View } from "react-native";
import * as Haptics from "expo-haptics";
import {
  resolveLabel,
  type CheckState,
  type Compatibility,
  type ConflictView,
  type PrCheckItem,
  type PrEntry,
  type PrState,
  type ResolveStrategy,
  type WorkspaceInfo,
} from "@zwiim/sdk";
import {
  continueError,
  continueLine,
  mergeError,
  offersContinue,
  prErrorText,
  prLabel,
  prLook,
  resolveError,
  shortSha,
  stopResolveError,
  type PrTone,
} from "../host/pr";
import { githubSignInOffered } from "../host/github";
import { currentHost } from "../services";
import { Button, Sheet, T } from "../ui/components";
import { ChevronRight, GitBranchPlus, GitMerge, GitPullRequest, Icon } from "../ui/icons";
import { space, useTheme, type Palette } from "../ui/tokens";

export const toneColor = (tone: PrTone, theme: Palette) =>
  ({ done: theme.done, err: theme.err, work: theme.mut, mut: theme.mut })[tone];

const CHECK_GLYPH: Record<CheckState, [string, (t: Palette) => string]> = {
  passed: ["✓", (t) => t.done],
  failed: ["✕", (t) => t.err],
  pending: ["●", (t) => t.mut],
  skipped: ["–", (t) => t.faint],
};

const open = (url: string | null) => {
  if (url && /^https:\/\//.test(url)) void Linking.openURL(url).catch(() => {});
};

export function PrStrip({
  workspace_id,
  entry,
  workspace,
  compat,
  conflicts,
  connected,
  hostName,
}: {
  workspace_id: string;
  entry: PrEntry | undefined;
  workspace: WorkspaceInfo | undefined;
  compat: Compatibility | null;
  /** The branch's conflicts with its base, or null when there are none to show. */
  conflicts: ConflictView | null;
  connected: boolean;
  hostName: string;
}) {
  const theme = useTheme();
  const [sheet, setSheet] = useState(false);
  /** The PR this phone just merged, shown merged until the feed says so too. */
  const [justMerged, setJustMerged] = useState<number | null>(null);
  /** The PR this phone just continued past, gone until the feed clears it. */
  const [continued, setContinued] = useState<number | null>(null);
  const feedPr = entry?.pr ?? null;
  useEffect(() => {
    setJustMerged(null);
    setContinued(null);
  }, [workspace_id, feedPr?.number]);
  const pr: PrState | null =
    feedPr && justMerged === feedPr.number && feedPr.state !== "merged" ? { ...feedPr, state: "merged" } : feedPr;

  // A continued PR is behind the worktree; its conflicts are the new branch's.
  const gone = !pr || (feedPr !== null && continued === feedPr.number);
  if (gone && conflicts) {
    return (
      <>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={conflictText(conflicts)}
          onPress={() => setSheet(true)}
          style={({ pressed }) => [styles.strip, { borderBottomColor: theme.line }, pressed && { backgroundColor: theme.surface }]}
          testID="pr-strip"
        >
          <Icon icon={GitMerge} color={theme.accent} size={16} />
          <T variant="callout" color={theme.fg} numberOfLines={1} style={styles.flex} testID="pr-strip-conflicts">
            {conflictText(conflicts)}
          </T>
          <Icon icon={ChevronRight} color={theme.faint} size={16} />
        </Pressable>
        <Sheet visible={sheet} onClose={() => setSheet(false)}>
          <T variant="title">Merge conflicts</T>
          <ConflictBlock workspace_id={workspace_id} view={conflicts} hasPr={false} connected={connected} hostName={hostName} />
          {entry?.error && (
            <T variant="monoSm" color={theme.mut} testID="pr-strip-error">
              {prErrorText(entry.error.code, entry.error.message, hostName, githubSignInOffered(compat))}
            </T>
          )}
        </Sheet>
      </>
    );
  }
  if (!entry) return null;
  if (feedPr && continued === feedPr.number) return null;
  if (!pr) {
    if (!entry.error) return null;
    return (
      <View style={[styles.strip, { borderBottomColor: theme.line }]} testID="pr-strip">
        <Icon icon={GitPullRequest} color={theme.faint} size={16} />
        <T variant="monoSm" color={theme.mut} numberOfLines={2} style={styles.flex} testID="pr-strip-error">
          {prErrorText(entry.error.code, entry.error.message, hostName, githubSignInOffered(compat))}
        </T>
      </View>
    );
  }

  const look = prLook(pr, connected);
  const color = toneColor(look.tone, theme);
  // A merged or closed PR keeps Continue; its branch's conflicts no longer matter.
  const conflict = look.settled ? null : conflicts;
  return (
    <>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={conflict ? `${prLabel(pr.number, look)}, ${conflictText(conflict)}` : prLabel(pr.number, look)}
        onPress={() => setSheet(true)}
        style={({ pressed }) => [styles.strip, { borderBottomColor: theme.line }, pressed && { backgroundColor: theme.surface }]}
        testID="pr-strip"
      >
        <Icon icon={pr.state === "merged" ? GitMerge : GitPullRequest} color={color} size={16} />
        <T variant="mono" color={theme.fg}>{`#${pr.number}`}</T>
        <T variant="callout" color={color} testID="pr-strip-status">
          {look.status}
        </T>
        {look.settled ? <View style={styles.flex} /> : (
          <T variant="monoSm" color={theme.mut} numberOfLines={1} style={styles.flex} testID="pr-strip-checks">
            {conflict ? conflictText(conflict) : look.checks}
          </T>
        )}
        <Icon icon={ChevronRight} color={theme.faint} size={16} />
      </Pressable>
      <PrSheet
        visible={sheet}
        pr={pr}
        workspace_id={workspace_id}
        connected={connected}
        hostName={hostName}
        conflicts={conflict}
        canContinue={offersContinue(pr, workspace, compat)}
        signInOffered={githubSignInOffered(compat)}
        onMerged={setJustMerged}
        onContinued={setContinued}
        onClose={() => setSheet(false)}
      />
    </>
  );
}

function PrSheet({
  visible,
  pr,
  workspace_id,
  connected,
  hostName,
  conflicts,
  canContinue,
  signInOffered,
  onMerged,
  onContinued,
  onClose,
}: {
  visible: boolean;
  pr: PrState;
  workspace_id: string;
  connected: boolean;
  hostName: string;
  conflicts: ConflictView | null;
  canContinue: boolean;
  /** A `gh_unauthenticated` refusal points to the Host tab's GitHub group. */
  signInOffered: boolean;
  onMerged(number: number): void;
  onContinued(number: number): void;
  onClose(): void;
}) {
  const theme = useTheme();
  const [confirm, setConfirm] = useState(false);
  const [merging, setMerging] = useState(false);
  const [continuing, setContinuing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const look = prLook(pr, connected);
  // A new head is a new pull request to judge.
  useEffect(() => {
    setError(null);
    setConfirm(false);
  }, [pr.head_sha, pr.state]);

  const close = () => {
    setConfirm(false);
    setError(null);
    onClose();
  };
  const merge = async () => {
    setMerging(true);
    setError(null);
    try {
      // Pinned to the head this sheet shows: a push since then is refused, not merged.
      const merged = await currentHost.mergePr(workspace_id, pr, pr.merge.method ?? undefined);
      void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success).catch(() => {});
      onMerged(merged.number);
      setConfirm(false);
    } catch (e) {
      setError(mergeError(e, hostName, signInOffered));
      setConfirm(false);
    } finally {
      setMerging(false);
    }
  };

  // No confirm: nothing is lost. Sessions, terminals and the old branch stay.
  const continueHere = async () => {
    setContinuing(true);
    setError(null);
    try {
      await currentHost.continueWorkspace(workspace_id, pr.number);
      void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success).catch(() => {});
      close();
      onContinued(pr.number);
    } catch (e) {
      setError(continueError(e, pr.number, hostName, signInOffered));
    } finally {
      setContinuing(false);
    }
  };

  if (confirm) {
    return (
      <Sheet visible={visible} onClose={close}>
        <T variant="title">{`Merge #${pr.number}?`}</T>
        <T color={theme.mut} testID="pr-confirm-body">
          {`${look.method ?? "Merge"} into ${pr.base} at ${shortSha(pr.head_sha)}. GitHub merges it now, and only if nothing changed since; the branch stays.`}
        </T>
        <Button tall kind="done" label="Merge" icon={GitMerge} loading={merging} onPress={() => void merge()} testID="pr-merge-confirm" />
        <Button tall kind="secondary" label="Cancel" onPress={() => setConfirm(false)} />
      </Sheet>
    );
  }

  return (
    <Sheet visible={visible} onClose={close}>
      <T variant="title" numberOfLines={3}>
        {pr.title}
      </T>
      <T variant="mono" color={theme.mut} numberOfLines={1} ellipsizeMode="middle">
        {`#${pr.number} · ${pr.base} ← ${pr.head}`}
      </T>
      <View style={styles.statusRow}>
        <T variant="callout" color={toneColor(look.tone, theme)} testID="pr-sheet-status">
          {look.status}
        </T>
        {look.settled ? null : (
          <T variant="monoSm" color={theme.mut}>
            {look.checks}
          </T>
        )}
      </View>
      {pr.checks.items.length > 0 && !look.settled ? (
        <ScrollView style={styles.checks} testID="pr-checks">
          {pr.checks.items.map((c, i) => (
            <CheckRow key={`${c.workflow ?? ""}/${c.name}/${i}`} check={c} />
          ))}
        </ScrollView>
      ) : null}
      {error ? (
        <T color={theme.err} testID="pr-merge-error">
          {error}
        </T>
      ) : null}
      {look.settled ? (
        canContinue ? (
          <>
            <T variant="callout" color={theme.mut} testID="pr-continue-line">
              {continueLine(pr)}
            </T>
            <Button
              tall
              kind="primary"
              label="Continue"
              icon={GitBranchPlus}
              loading={continuing}
              disabled={!connected}
              onPress={() => void continueHere()}
              testID="pr-continue"
            />
          </>
        ) : null
      ) : (
        <>
          {conflicts ? (
            <ConflictBlock workspace_id={workspace_id} view={conflicts} hasPr connected={connected} hostName={hostName} />
          ) : null}
          {conflicts?.line ? null : look.reason ? (
            <T variant="callout" color={theme.mut} testID="pr-merge-reason">
              {look.reason}
            </T>
          ) : look.method ? (
            <T variant="monoSm" color={theme.mut}>
              {`${look.method} into ${pr.base}`}
            </T>
          ) : null}
          {conflicts?.resolve ? null : (
            <Button
              tall
              kind="done"
              label="Merge"
              icon={GitMerge}
              disabled={!look.canMerge}
              onPress={() => setConfirm(true)}
              testID="pr-merge"
            />
          )}
        </>
      )}
      <Button tall kind="secondary" label="Open on GitHub" onPress={() => open(pr.url)} testID="pr-open" />
    </Sheet>
  );
}

/** The strip's one line: the conflicts, or the turn still resolving once they cleared. */
const conflictText = (view: ConflictView) => view.line ?? "Resolving conflicts…";

/**
 * The conflict line, its files and the resolve buttons. No confirm: the Host
 * checkpoints before the agent's turn, so the turn can be undone.
 */
function ConflictBlock({
  workspace_id,
  view,
  hasPr,
  connected,
  hostName,
}: {
  workspace_id: string;
  view: ConflictView;
  /** `Merge conflicts` stands where Merge was; without a PR, `Resolve conflicts`. */
  hasPr: boolean;
  connected: boolean;
  hostName: string;
}) {
  const theme = useTheme();
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const action = view.resolve;
  const resolve = async (strategy?: ResolveStrategy) => {
    if (action?.state !== "ready") return;
    setStarting(true);
    setError(null);
    try {
      await currentHost.resolveConflicts(workspace_id, action.session_id, strategy);
      void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success).catch(() => {});
    } catch (e) {
      setError(resolveError(e, hostName));
    } finally {
      setStarting(false);
    }
  };
  const stop = async () => {
    if (action?.state !== "resolving") return;
    setError(null);
    try {
      await currentHost.cancel(action.session_id);
    } catch (e) {
      setError(stopResolveError(e, hostName));
    }
  };
  return (
    <>
      {view.line ? (
        <T variant="callout" color={theme.fg} testID="conflict-line">
          {view.line}
        </T>
      ) : null}
      {view.files.length > 0 ? (
        <ScrollView style={styles.files} accessibilityLabel="Conflicting files" testID="conflict-files">
          {view.files.map((f) => (
            <T key={f} variant="mono" color={theme.mut} numberOfLines={1} ellipsizeMode="middle" style={styles.file}>
              {f}
            </T>
          ))}
        </ScrollView>
      ) : null}
      {action?.state === "blocked" ? (
        <T variant="callout" color={theme.mut} testID="conflict-blocked">
          {action.reason}
        </T>
      ) : null}
      {error ? (
        <T color={theme.err} testID="conflict-error">
          {error}
        </T>
      ) : null}
      {action ? (
        <>
          <Button
            tall
            kind="primary"
            label={resolveLabel(action, hasPr)}
            icon={GitMerge}
            loading={starting}
            disabled={action.state !== "ready" || !connected}
            onPress={() => void resolve()}
            testID="conflict-resolve"
          />
          {action.state === "resolving" ? (
            <Button tall kind="secondary" label="Cancel" disabled={!connected} onPress={() => void stop()} testID="conflict-cancel" />
          ) : (
            <Button
              tall
              kind="secondary"
              label="Rebase instead"
              disabled={action.state !== "ready" || !connected || starting}
              onPress={() => void resolve("rebase")}
              testID="conflict-rebase"
            />
          )}
        </>
      ) : null}
    </>
  );
}

function CheckRow({ check }: { check: PrCheckItem }) {
  const theme = useTheme();
  const [glyph, color] = CHECK_GLYPH[check.state];
  return (
    <Pressable
      accessibilityRole="link"
      accessibilityLabel={`${check.name}, ${check.state}`}
      disabled={!check.link}
      onPress={() => open(check.link)}
      style={({ pressed }) => [styles.check, { borderBottomColor: theme.line }, pressed && { backgroundColor: theme.surface }]}
    >
      <T variant="mono" color={color(theme)} style={styles.glyph}>
        {glyph}
      </T>
      <T variant="mono" numberOfLines={1} style={styles.flex}>
        {check.workflow ? `${check.workflow} / ${check.name}` : check.name}
      </T>
      {check.link ? <Icon icon={ChevronRight} color={theme.faint} size={14} /> : null}
    </Pressable>
  );
}

const styles = StyleSheet.create({
  strip: {
    minHeight: 40,
    flexDirection: "row",
    alignItems: "center",
    gap: space.sm,
    paddingHorizontal: space.lg,
    paddingVertical: space.xs,
    borderBottomWidth: StyleSheet.hairlineWidth,
  },
  flex: { flex: 1 },
  statusRow: { flexDirection: "row", alignItems: "center", gap: space.sm },
  checks: { maxHeight: 240 },
  files: { maxHeight: 160 },
  file: { paddingVertical: space.xs },
  check: {
    minHeight: 44,
    flexDirection: "row",
    alignItems: "center",
    gap: space.sm,
    borderBottomWidth: StyleSheet.hairlineWidth,
  },
  glyph: { width: 16, textAlign: "center" },
});
