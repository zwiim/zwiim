// The PR strip (DESIGN.md, Screen 5): under the session strip, the worktree
// branch's pull request as the Host follows it on GitHub. One line: `#12`,
// its status and checks; it opens the PR sheet (title, base ← head, each
// check, Merge). Merge turns the sheet into its confirm, and the Host merges
// at the head shown there or refuses. Once merged, Continue moves the worktree
// to a new branch from the PR's base, and the strip goes. The phone never asks
// GitHub.

import { useEffect, useState } from "react";
import { Linking, Pressable, ScrollView, StyleSheet, View } from "react-native";
import * as Haptics from "expo-haptics";
import type { CheckState, Compatibility, PrCheckItem, PrEntry, PrState, WorkspaceInfo } from "@zwiim/sdk";
import {
  continueError,
  continueLine,
  mergeError,
  offersContinue,
  prErrorText,
  prLabel,
  prLook,
  shortSha,
  type PrTone,
} from "../host/pr";
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
  connected,
  hostName,
}: {
  workspace_id: string;
  entry: PrEntry | undefined;
  workspace: WorkspaceInfo | undefined;
  compat: Compatibility | null;
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

  if (!entry) return null;
  if (feedPr && continued === feedPr.number) return null;
  if (!pr) {
    if (!entry.error) return null;
    return (
      <View style={[styles.strip, { borderBottomColor: theme.line }]} testID="pr-strip">
        <Icon icon={GitPullRequest} color={theme.faint} size={16} />
        <T variant="monoSm" color={theme.mut} numberOfLines={2} style={styles.flex} testID="pr-strip-error">
          {prErrorText(entry.error.code, entry.error.message, hostName)}
        </T>
      </View>
    );
  }

  const look = prLook(pr, connected);
  const color = toneColor(look.tone, theme);
  return (
    <>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={prLabel(pr.number, look)}
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
          <T variant="monoSm" color={theme.mut} numberOfLines={1} style={styles.flex}>
            {look.checks}
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
        canContinue={offersContinue(pr, workspace, compat)}
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
  canContinue,
  onMerged,
  onContinued,
  onClose,
}: {
  visible: boolean;
  pr: PrState;
  workspace_id: string;
  connected: boolean;
  hostName: string;
  canContinue: boolean;
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
      setError(mergeError(e, hostName));
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
      setError(continueError(e, pr.number, hostName));
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
          {look.reason ? (
            <T variant="callout" color={theme.mut} testID="pr-merge-reason">
              {look.reason}
            </T>
          ) : look.method ? (
            <T variant="monoSm" color={theme.mut}>
              {`${look.method} into ${pr.base}`}
            </T>
          ) : null}
          <Button
            tall
            kind="done"
            label="Merge"
            icon={GitMerge}
            disabled={!look.canMerge}
            onPress={() => setConfirm(true)}
            testID="pr-merge"
          />
        </>
      )}
      <Button tall kind="secondary" label="Open on GitHub" onPress={() => open(pr.url)} testID="pr-open" />
    </Sheet>
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
  check: {
    minHeight: 44,
    flexDirection: "row",
    alignItems: "center",
    gap: space.sm,
    borderBottomWidth: StyleSheet.hairlineWidth,
  },
  glyph: { width: 16, textAlign: "center" },
});
