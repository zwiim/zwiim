// Archiving (removing) a worktree, or removing a whole workspace, inside the
// workspace sheet. The Host is the authority: it answers each attempt, and what
// it answers is shown. While agents or terminals run there it names the
// sessions; their agents can be stopped from here, and removal is asked again
// only once every stop succeeded. Uncommitted changes are discarded only when
// confirmed. A workspace's folder on the host is never touched.

import { useState, type ReactNode } from "react";
import { StyleSheet, View } from "react-native";
import type { WorkspaceInfo } from "@zwiim/sdk";
import { useHostState } from "../app/hooks";
import {
  otherBlockers,
  removalError,
  removeWorkspace,
  removeWorktree,
  stopAgentError,
  stopThenRemove,
  workspaceRemovalError,
  type RemovalStep,
} from "../host/removal";
import { workspaceLabel } from "../host/workspace";
import { sessionItem, sessionLook } from "../sessions/model";
import { currentHost } from "../services";
import { Button, StatusDot, T } from "../ui/components";
import { radius, space, useTheme } from "../ui/tokens";
import { useHostName } from "./HostFrame";

type Shown = { step: "confirm" } | Exclude<RemovalStep, { step: "removed" }>;

const plural = (n: number, one: string, many: string) => (n === 1 ? `1 ${one}` : `${n} ${many}`);

/** What one removal says at each step, and how it asks the Host. */
interface Removal {
  /** `removeWorktree` or `removeWorkspace`. */
  ask: typeof removeWorktree;
  error(e: unknown): string;
  confirmTitle: string;
  confirmText: ReactNode;
  confirmLabel: string;
  busyTitle: string;
  busyText: string;
  /** "archive" or "remove", after "Stop n agents and". */
  verb: string;
  dirtyText: string;
}

export function RemoveWorktree({ worktree, onDone, onCancel }: { worktree: WorkspaceInfo; onDone(): void; onCancel(): void }) {
  const hostName = useHostName();
  const branch = `⎇ ${worktree.branch ?? workspaceLabel(worktree)}`;
  const removal: Removal = {
    ask: removeWorktree,
    error: (e) => removalError(e, hostName),
    confirmTitle: `Archive ${branch}?`,
    confirmText: `Its checkout is deleted from ${hostName} and its sessions stay readable. The branch stays unless ${hostName} is set to delete branches that merged or are pushed.`,
    confirmLabel: "Archive",
    busyTitle: `Still running in ${branch}`,
    busyText: `${hostName} won't archive a worktree while agents or terminals run there.`,
    verb: "archive",
    dirtyText: `${branch} has changes that aren't committed. Archiving it anyway deletes them; committed work is kept.`,
  };
  return <RemoveFlow removal={removal} workspace_id={worktree.workspace_id} onDone={onDone} onCancel={onCancel} />;
}

/** Remove a workspace (a repository or a plain folder): its worktrees are archived, its folder stays. */
export function RemoveWorkspace({ workspace, onDone, onCancel }: { workspace: WorkspaceInfo; onDone(): void; onCancel(): void }) {
  const theme = useTheme();
  const hostName = useHostName();
  const { workspaces } = useHostState();
  const name = workspaceLabel(workspace);
  const worktrees = (workspaces ?? []).filter((w) => w.parent_workspace_id === workspace.workspace_id && !w.removed).length;
  const removal: Removal = {
    ask: removeWorkspace,
    error: (e) => workspaceRemovalError(e, hostName),
    confirmTitle: `Remove ${name}?`,
    confirmText: (
      <>
        {worktrees === 1 ? "Its worktree is archived. " : worktrees > 1 ? `Its ${worktrees} worktrees are archived. ` : ""}
        Files in{" "}
        {workspace.display_path ? (
          <T variant="mono" color={theme.mut}>
            {workspace.display_path}
          </T>
        ) : (
          "its folder"
        )}{" "}
        on {hostName} aren't changed. Add the folder again to bring it back, sessions and all.
      </>
    ),
    confirmLabel: "Remove",
    busyTitle: `Still running in ${name}`,
    busyText: `${hostName} won't remove a workspace while agents run in it.`,
    verb: "remove",
    dirtyText: `Its worktrees have changes that aren't committed. Removing it anyway deletes them; committed work is kept.`,
  };
  return <RemoveFlow removal={removal} workspace_id={workspace.workspace_id} onDone={onDone} onCancel={onCancel} />;
}

function RemoveFlow({
  removal,
  workspace_id,
  onDone,
  onCancel,
}: {
  removal: Removal;
  workspace_id: string;
  onDone(): void;
  onCancel(): void;
}) {
  const theme = useTheme();
  const hostName = useHostName();
  const state = useHostState();
  const [shown, setShown] = useState<Shown>({ step: "confirm" });
  const [working, setWorking] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const run = async (attempt: () => Promise<RemovalStep>, failure: (e: unknown) => string) => {
    setWorking(true);
    setError(null);
    try {
      const next = await attempt();
      if (next.step === "removed") onDone();
      else setShown(next);
    } catch (e) {
      setError(failure(e));
    } finally {
      setWorking(false);
    }
  };
  const remove = (force = false) => run(() => removal.ask(currentHost, workspace_id, force), removal.error);

  let body;
  switch (shown.step) {
    case "confirm":
      body = (
        <>
          <T variant="title">{removal.confirmTitle}</T>
          <T color={theme.mut}>{removal.confirmText}</T>
          {error ? <ErrorText text={error} /> : null}
          <Button tall kind="destructive" label={removal.confirmLabel} loading={working} onPress={() => void remove()} testID="remove-confirm" />
        </>
      );
      break;
    case "busy": {
      const { session_ids } = shown.busy;
      const other = otherBlockers(shown.busy);
      const stop = () =>
        run(
          () => stopThenRemove(currentHost, workspace_id, session_ids, false, removal.ask),
          (e) => stopAgentError(e, hostName),
        );
      body = (
        <>
          <T variant="title">{removal.busyTitle}</T>
          <T color={theme.mut}>{removal.busyText}</T>
          <View style={[styles.list, { backgroundColor: theme.surface }]} testID="remove-blockers">
            {session_ids.map((id) => {
              const item = sessionItem(state, id);
              return (
                <View key={id} style={styles.row} testID={`remove-blocker-${id}`}>
                  <StatusDot state={item ? sessionLook(item.state).dot : "work"} />
                  <View style={styles.flex}>
                    <T numberOfLines={1}>{item?.title ?? "Session"}</T>
                    {item?.agent ? (
                      <T variant="monoSm" color={theme.mut} numberOfLines={1}>
                        {item.agent} · agent running
                      </T>
                    ) : null}
                  </View>
                </View>
              );
            })}
            {other ? (
              <T variant="monoSm" color={theme.mut} style={styles.row} testID="remove-other">
                {other}
              </T>
            ) : null}
          </View>
          {shown.busy.terminals > 0 ? (
            <T color={theme.mut}>Kill its terminals from Terminals in Files, then check again.</T>
          ) : null}
          {session_ids.length ? (
            <T color={theme.mut}>
              Stopping an agent also stops its background work. Each session and its transcript stay.
            </T>
          ) : null}
          {error ? <ErrorText text={error} /> : null}
          {session_ids.length ? (
            <Button
              tall
              kind="destructive"
              label={`Stop ${plural(session_ids.length, "agent", "agents")} and ${removal.verb}`}
              loading={working}
              onPress={() => void stop()}
              testID="remove-stop-agents"
            />
          ) : (
            <Button tall kind="secondary" label="Check again" loading={working} onPress={() => void remove()} testID="remove-again" />
          )}
        </>
      );
      break;
    }
    case "dirty":
      body = (
        <>
          <T variant="title">Discard {plural(shown.changes, "uncommitted change", "uncommitted changes")}?</T>
          <T color={theme.mut}>{removal.dirtyText}</T>
          {error ? <ErrorText text={error} /> : null}
          <Button
            tall
            kind="destructive"
            label={`Discard and ${removal.verb}`}
            loading={working}
            onPress={() => void remove(true)}
            testID="remove-force"
          />
        </>
      );
      break;
  }
  return (
    <>
      {body}
      <Button tall kind="secondary" label="Cancel" disabled={working} onPress={onCancel} testID="remove-cancel" />
    </>
  );
}

function ErrorText({ text }: { text: string }) {
  const theme = useTheme();
  return (
    <T color={theme.err} testID="remove-error">
      {text}
    </T>
  );
}

const styles = StyleSheet.create({
  flex: { flex: 1 },
  list: { borderRadius: radius.lg, paddingVertical: space.xs },
  row: { flexDirection: "row", alignItems: "center", gap: space.sm, paddingHorizontal: space.md, paddingVertical: space.sm },
});
