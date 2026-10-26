// GitHub projects on the phone: the New workspace chooser, the repository
// list that starts clones, and the clone cards above the workspace list.
// Progress always comes from the Host's clone jobs; nothing here copies it.

import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { ActivityIndicator, FlatList, Pressable, RefreshControl, StyleSheet, TextInput, View } from "react-native";
import Svg, { Path } from "react-native-svg";
import { fullName, hasCapability, isActive, progressLabel, sameRepo, type CloneJob, type GithubRepo } from "@zwiim/sdk";
import { useCurrentHost, useHostState, useNow } from "../app/hooks";
import { addedWorkspace, cloneCards, cloneProblem, listProblem, repoMeta, repoRows, settleClones, type RepoRow } from "../host/clone";
import { githubSignInOffered } from "../host/github";
import { currentHost } from "../services";
import { Button, T } from "../ui/components";
import { ChevronLeft, ChevronRight, Folder, Icon, Lock, RotateCw } from "../ui/icons";
import { radius, space, type, useTheme } from "../ui/tokens";
import { useHostName } from "./HostFrame";

/** GitHub's mark: Primer Octicons `mark-github` (MIT). Lucide has none. */
export function GithubMark({ color, size = 20 }: { color: string; size?: number }) {
  return (
    <Svg width={size} height={size} viewBox="0 0 16 16" fill={color}>
      <Path d="M8 0c4.42 0 8 3.58 8 8a8.013 8.013 0 0 1-5.45 7.59c-.4.08-.55-.17-.55-.38 0-.27.01-1.13.01-2.2 0-.75-.25-1.23-.54-1.48 1.78-.2 3.65-.88 3.65-3.95 0-.88-.31-1.59-.82-2.15.08-.2.36-1.02-.08-2.12 0 0-.67-.22-2.2.82-.64-.18-1.32-.27-2-.27-.68 0-1.36.09-2 .27-1.53-1.03-2.2-.82-2.2-.82-.44 1.1-.16 1.92-.08 2.12-.51.56-.82 1.28-.82 2.15 0 3.06 1.86 3.75 3.64 3.95-.23.2-.44.55-.51 1.07-.46.21-1.61.55-2.33-.66-.15-.24-.6-.83-1.23-.82-.67.01-.27.38.01.53.34.19.73.9.82 1.13.16.45.68 1.31 2.69.94 0 .67.01 1.3.01 1.49 0 .21-.15.45-.55.38A7.995 7.995 0 0 1 0 8c0-4.42 3.58-8 8-8Z" />
    </Svg>
  );
}

/** A thin accent bar; an unknown percent shows the track only. */
export function ProgressBar({ percent }: { percent: number | null }) {
  const theme = useTheme();
  return (
    <View style={[styles.track, { backgroundColor: theme.line }]}>
      {percent !== null ? <View style={[styles.fill, { width: `${percent}%`, backgroundColor: theme.accent }]} /> : null}
    </View>
  );
}

/** Whether this Host can clone; without it New workspace goes straight to Browse Mac. */
export function useCanClone(): boolean {
  const { view } = useCurrentHost();
  return hasCapability(view?.compat ?? null, "workspace.clone");
}

/**
 * Clones a sheet started, waited on until done. Returns the function that
 * adds one; `onDone` runs once per clone that ends done.
 */
export function useStartedClones(onDone: (job: CloneJob) => void): (clone_id: string) => void {
  const { clones } = useHostState();
  const [ids, setIds] = useState<readonly string[]>([]);
  const done = useRef(onDone);
  done.current = onDone;
  useEffect(() => {
    if (ids.length === 0) return;
    const settled = settleClones(clones, ids);
    if (settled.left.length !== ids.length) setIds(settled.left);
    for (const job of settled.done) done.current(job);
  }, [ids, clones]);
  return useCallback((clone_id: string) => setIds((was) => [...was, clone_id]), []);
}

function ChoiceRow({
  glyph,
  title,
  sub,
  disabled,
  onPress,
  testID,
}: {
  glyph: ReactNode;
  title: string;
  sub: string;
  disabled?: boolean;
  onPress(): void;
  testID: string;
}) {
  const theme = useTheme();
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityState={{ disabled: !!disabled }}
      disabled={disabled}
      onPress={onPress}
      style={({ pressed }) => [styles.choice, { opacity: disabled ? 0.5 : 1 }, pressed && { backgroundColor: theme.surface }]}
      testID={testID}
    >
      <View style={[styles.tile, { backgroundColor: theme.surface }]}>{glyph}</View>
      <View style={styles.flex}>
        <T variant="headline">{title}</T>
        <T variant="monoSm" color={theme.mut}>
          {sub}
        </T>
      </View>
      <Icon icon={ChevronRight} color={theme.faint} />
    </Pressable>
  );
}

/** "New workspace": a folder on the Mac, or a GitHub project. */
export function NewWorkspaceChooser({ onFolder, onGithub }: { onFolder(): void; onGithub(): void }) {
  const theme = useTheme();
  const { view } = useCurrentHost();
  return (
    <>
      <T variant="title" style={styles.title}>
        New workspace
      </T>
      <ChoiceRow
        glyph={<Icon icon={Folder} color={theme.fg} />}
        title="Folder on Mac"
        sub="pick any folder in ~/"
        onPress={onFolder}
        testID="new-workspace-folder"
      />
      <ChoiceRow
        glyph={<GithubMark color={theme.fg} />}
        title="GitHub project"
        sub="clone into ~/zwiim/repos"
        disabled={view?.status !== "live"}
        onPress={onGithub}
        testID="new-workspace-github"
      />
    </>
  );
}

type List =
  | { kind: "loading" }
  | { kind: "failed"; text: string }
  | { kind: "ready"; repos: GithubRepo[]; next_page: number | null };

/** The Host's GitHub repositories: search or paste, tap to clone or open. */
export function GithubProjects({
  onBack,
  onOpen,
  choose,
  onStarted,
}: {
  onBack(): void;
  /** A repository already added: its root workspace. */
  onOpen(workspace_id: string): void;
  /** Whether the clone's worktree becomes the Files selection when done. */
  choose: boolean;
  onStarted(clone_id: string): void;
}) {
  const theme = useTheme();
  const host = useHostName();
  const { view } = useCurrentHost();
  const signIn = githubSignInOffered(view?.compat ?? null);
  const { clones } = useHostState();
  const live = view?.status === "live";
  const now = Math.floor(useNow(false) / 1000);
  const [query, setQuery] = useState("");
  const [list, setList] = useState<List>({ kind: "loading" });
  const [refreshing, setRefreshing] = useState(false);
  const [more, setMore] = useState<{ loading: boolean; error: string | null }>({ loading: false, error: null });
  const [starting, setStarting] = useState<ReadonlySet<string>>(new Set());
  const [refused, setRefused] = useState<{ key: string; text: string } | null>(null);
  // Each load bumps it; an answer for an older load is dropped.
  const asked = useRef(0);

  const load = async (refresh: boolean) => {
    const n = ++asked.current;
    if (refresh) setRefreshing(true);
    else setList({ kind: "loading" });
    setMore({ loading: false, error: null });
    try {
      const page = await currentHost.githubRepos(1, refresh);
      if (n === asked.current) setList({ kind: "ready", repos: page.repos, next_page: page.next_page });
    } catch (e) {
      if (n === asked.current) setList({ kind: "failed", text: listProblem(e, host, signIn) });
    } finally {
      if (n === asked.current) setRefreshing(false);
    }
  };

  const loadMore = async () => {
    if (list.kind !== "ready" || list.next_page === null || more.loading) return;
    const n = asked.current;
    setMore({ loading: true, error: null });
    try {
      const page = await currentHost.githubRepos(list.next_page);
      if (n !== asked.current) return;
      setList((l) =>
        l.kind === "ready"
          ? { kind: "ready", repos: [...l.repos, ...page.repos.filter((r) => !l.repos.some((o) => sameRepo(o, r)))], next_page: page.next_page }
          : l,
      );
      setMore({ loading: false, error: null });
    } catch (e) {
      if (n === asked.current) setMore({ loading: false, error: listProblem(e, host, signIn) });
    }
  };

  // Page 1 once there is a connection; loaded rows stay if it drops.
  useEffect(() => {
    if (live && list.kind !== "ready") void load(false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [live]);

  const clone = async (row: RepoRow) => {
    const target = row.kind === "repo" ? row.repo : row.ref;
    const key = fullName(target).toLowerCase();
    if (!live || starting.has(key)) return;
    setStarting((s) => new Set(s).add(key));
    setRefused(null);
    try {
      const started = await currentHost.startClone(target.owner, target.name, { choose });
      if (row.job?.state === "failed") currentHost.dismissClone(row.job.clone_id);
      if ("workspace_id" in started) onOpen(started.workspace_id);
      else onStarted(started.clone_id);
    } catch (e) {
      setRefused({ key, text: listProblem(e, host, signIn) });
    } finally {
      setStarting((s) => {
        const next = new Set(s);
        next.delete(key);
        return next;
      });
    }
  };

  const tap = (row: RepoRow) => {
    const added = addedWorkspace(row);
    if (added) return onOpen(added);
    if (row.job && isActive(row.job)) return;
    void clone(row);
  };

  const cancel = async (key: string, clone_id: string) => {
    try {
      await currentHost.cancelClone(clone_id);
    } catch (e) {
      setRefused({ key, text: listProblem(e, host, signIn) });
    }
  };

  const jobs = Object.values(clones);
  const rows = list.kind === "ready" ? repoRows(list.repos, query, jobs) : [];
  const nextPage = list.kind === "ready" ? list.next_page : null;

  const empty =
    list.kind === "loading" ? (
      <ActivityIndicator color={theme.mut} style={styles.none} />
    ) : list.kind === "failed" ? (
      <View style={styles.none}>
        <T color={theme.mut}>{list.text}</T>
        <Button kind="secondary" label="Try again" disabled={!live} onPress={() => void load(true)} testID="github-retry-list" />
      </View>
    ) : list.repos.length === 0 && !query.trim() ? (
      <T color={theme.mut} style={styles.none}>
        No repositories on this GitHub account. Paste owner/name or a GitHub link above.
      </T>
    ) : (
      <T color={theme.mut} style={styles.none}>
        No match in loaded repositories
      </T>
    );

  return (
    <>
      <View style={styles.head}>
        <Pressable accessibilityRole="button" accessibilityLabel="Back" onPress={onBack} hitSlop={space.sm} testID="github-back">
          <Icon icon={ChevronLeft} color={theme.fg} />
        </Pressable>
        <T variant="title" style={styles.flex}>
          GitHub project
        </T>
      </View>
      <TextInput
        value={query}
        onChangeText={setQuery}
        placeholder="Search, or paste owner/name"
        placeholderTextColor={theme.faint}
        autoCapitalize="none"
        autoCorrect={false}
        clearButtonMode="while-editing"
        style={[type.body, styles.search, { color: theme.fg, backgroundColor: theme.surface }]}
        accessibilityLabel="Search, or paste owner/name"
        testID="github-search"
      />
      {!live ? (
        <T color={theme.mut} style={styles.note}>
          Not connected to {host}.
        </T>
      ) : null}
      <FlatList
        data={rows}
        keyExtractor={(row) => (row.kind === "repo" ? fullName(row.repo) : `paste:${fullName(row.ref)}`).toLowerCase()}
        keyboardShouldPersistTaps="handled"
        refreshControl={<RefreshControl refreshing={refreshing} onRefresh={() => void load(true)} tintColor={theme.mut} />}
        ListEmptyComponent={empty}
        renderItem={({ item }) => {
          const target = item.kind === "repo" ? item.repo : item.ref;
          const key = fullName(target).toLowerCase();
          return (
            <RepoItem
              row={item}
              now={now}
              host={host}
              live={live}
              busy={starting.has(key)}
              refused={refused?.key === key ? refused.text : null}
              onPress={() => tap(item)}
              onRetry={() => void clone(item)}
              onCancel={(clone_id) => void cancel(key, clone_id)}
            />
          );
        }}
        ListFooterComponent={
          nextPage !== null ? (
            <View style={styles.more}>
              {more.error ? <T color={theme.err}>{more.error}</T> : null}
              <Button kind="ghost" label="Load more" loading={more.loading} disabled={!live} onPress={() => void loadMore()} testID="github-more" />
            </View>
          ) : null
        }
      />
    </>
  );
}

function Pill({ label, color, lock }: { label: string; color: string; lock?: boolean }) {
  return (
    <View style={[styles.pill, { borderColor: color }]}>
      {lock ? <Icon icon={Lock} color={color} size={11} /> : null}
      <T variant="monoSm" color={color}>
        {label}
      </T>
    </View>
  );
}

function RepoItem({
  row,
  now,
  host,
  live,
  busy,
  refused,
  onPress,
  onRetry,
  onCancel,
}: {
  row: RepoRow;
  now: number;
  host: string;
  live: boolean;
  busy: boolean;
  refused: string | null;
  onPress(): void;
  onRetry(): void;
  onCancel(clone_id: string): void;
}) {
  const theme = useTheme();
  const { view } = useCurrentHost();
  const target = row.kind === "repo" ? row.repo : row.ref;
  const repo = row.kind === "repo" ? row.repo : null;
  const added = addedWorkspace(row);
  const active = row.job && isActive(row.job) ? row.job : null;
  const failed =
    !added && !active && row.job?.state === "failed"
      ? cloneProblem(row.job, host, githubSignInOffered(view?.compat ?? null))
      : null;
  const pill = added
    ? <Pill label="Added" color={theme.accent} />
    : repo?.private
      ? <Pill label="Private" color={theme.mut} lock />
      : repo?.fork
        ? <Pill label="Fork" color={theme.mut} />
        : repo?.archived
          ? <Pill label="Archived" color={theme.mut} />
          : null;
  const meta = repo ? repoMeta(repo, now) : "";
  return (
    <Pressable
      accessibilityRole="button"
      disabled={!live || busy || !!active}
      onPress={onPress}
      style={({ pressed }) => [styles.row, { opacity: repo?.archived ? 0.55 : 1 }, pressed && { backgroundColor: theme.surface }]}
      testID={`github-repo-${fullName(target)}`}
    >
      <View style={styles.rowHead}>
        <T variant="mono" numberOfLines={1} style={styles.flex}>
          <T variant="mono" color={theme.mut}>
            {target.owner}/
          </T>
          {target.name}
        </T>
        {busy ? <ActivityIndicator color={theme.mut} /> : pill}
      </View>
      {repo?.description ? (
        <T variant="callout" color={theme.mut} numberOfLines={1}>
          {repo.description}
        </T>
      ) : row.kind === "paste" ? (
        <T variant="callout" color={theme.mut} numberOfLines={1}>
          Not in your list. Tap to clone.
        </T>
      ) : null}
      {meta ? (
        <T variant="monoSm" color={theme.mut}>
          {meta}
        </T>
      ) : null}
      {active ? (
        <View style={styles.progress}>
          <ProgressBar percent={active.percent} />
          <View style={styles.rowHead}>
            <T variant="callout" color={theme.mut} style={styles.flex}>
              {progressLabel(active)}
            </T>
            <Button kind="ghost" label="Cancel" disabled={!live} onPress={() => onCancel(active.clone_id)} />
          </View>
        </View>
      ) : null}
      {failed ? (
        <View style={styles.rowHead}>
          <T variant="callout" color={theme.err} style={styles.flex}>
            {failed.text}
          </T>
          {failed.retry ? <Button kind="ghost" icon={RotateCw} label="Retry" disabled={!live} onPress={onRetry} /> : null}
        </View>
      ) : null}
      {refused ? (
        <T variant="callout" color={theme.err}>
          {refused}
        </T>
      ) : null}
    </Pressable>
  );
}

/** Clones in flight and failures, above the workspace list. */
export function CloneCards({ onStarted }: { onStarted(clone_id: string): void }) {
  const theme = useTheme();
  const host = useHostName();
  const { view, dismissedClones } = useCurrentHost();
  const signIn = githubSignInOffered(view?.compat ?? null);
  const { clones } = useHostState();
  const live = view?.status === "live";
  const [error, setError] = useState<string | null>(null);
  const cards = cloneCards(clones, dismissedClones);
  if (cards.length === 0 && !error) return null;

  const retry = async (job: CloneJob) => {
    setError(null);
    try {
      const started = await currentHost.startClone(job.owner, job.name, { choose: true });
      currentHost.dismissClone(job.clone_id);
      if ("clone_id" in started) onStarted(started.clone_id);
      else currentHost.chooseWorkspace(started.workspace_id);
    } catch (e) {
      setError(listProblem(e, host, signIn));
    }
  };

  const cancel = async (clone_id: string) => {
    try {
      await currentHost.cancelClone(clone_id);
    } catch (e) {
      setError(listProblem(e, host, signIn));
    }
  };

  return (
    <View style={styles.cards}>
      {cards.map((job) => {
        const problem = job.state === "failed" ? cloneProblem(job, host, signIn) : null;
        return (
          <View key={job.clone_id} style={[styles.card, { backgroundColor: theme.surface }]} testID={`clone-${job.clone_id}`}>
            <View style={styles.rowHead}>
              <GithubMark color={theme.fg} size={16} />
              <T variant="mono" numberOfLines={1} style={styles.flex}>
                {fullName(job)}
              </T>
              {problem ? null : (
                <Button kind="ghost" label="Cancel" disabled={!live} onPress={() => void cancel(job.clone_id)} />
              )}
            </View>
            {problem ? (
              <>
                <T variant="callout" color={theme.err}>
                  {problem.text}
                </T>
                <View style={styles.rowHead}>
                  {problem.retry ? <Button kind="ghost" icon={RotateCw} label="Retry" disabled={!live} onPress={() => void retry(job)} /> : null}
                  <Button kind="ghost" label="Dismiss" onPress={() => currentHost.dismissClone(job.clone_id)} />
                </View>
              </>
            ) : (
              <>
                <ProgressBar percent={job.percent} />
                <T variant="callout" color={theme.mut}>
                  {progressLabel(job)}
                </T>
              </>
            )}
          </View>
        );
      })}
      {error ? (
        <T variant="callout" color={theme.err}>
          {error}
        </T>
      ) : null}
    </View>
  );
}

const styles = StyleSheet.create({
  flex: { flex: 1 },
  title: { marginBottom: space.md },
  head: { flexDirection: "row", alignItems: "center", gap: space.sm, marginBottom: space.md },
  search: { borderRadius: radius.md, paddingHorizontal: space.md, paddingVertical: space.sm, marginBottom: space.sm },
  note: { marginBottom: space.sm },
  none: { paddingVertical: space.lg, gap: space.md },
  more: { alignItems: "center", paddingVertical: space.md, gap: space.sm },
  choice: { flexDirection: "row", alignItems: "center", gap: space.md, paddingVertical: space.md, paddingHorizontal: space.sm, borderRadius: radius.md },
  tile: { width: 40, height: 40, borderRadius: radius.md, alignItems: "center", justifyContent: "center" },
  row: { paddingVertical: space.md, paddingHorizontal: space.sm, gap: space.xs, borderRadius: radius.md },
  rowHead: { flexDirection: "row", alignItems: "center", gap: space.sm },
  progress: { gap: space.xs, marginTop: space.xs },
  pill: { flexDirection: "row", alignItems: "center", gap: space.xxs, borderWidth: StyleSheet.hairlineWidth, borderRadius: radius.sm, paddingHorizontal: space.xs, paddingVertical: space.xxs },
  track: { height: 4, borderRadius: radius.full, overflow: "hidden" },
  fill: { height: 4, borderRadius: radius.full },
  cards: { gap: space.sm, marginBottom: space.md },
  card: { borderRadius: radius.md, padding: space.md, gap: space.xs },
});
