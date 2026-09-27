// Removing a worktree, inside the worktree sheet. The Host is the authority:
// it answers each attempt, and what it answers is shown. While agents or
// terminals run there it names the sessions; their agents can be stopped from
// here, and removal is asked again only once every stop succeeded. Uncommitted
// changes are discarded only when confirmed.

import { useState } from "react";
import { StyleSheet, View } from "react-native";
import type { WorkspaceInfo } from "@zwiim/sdk";
import { useHostState } from "../app/hooks";
import { otherBlockers, removalError, removeWorktree, stopAgentError, stopThenRemove, type RemovalStep } from "../host/removal";
import { workspaceLabel } from "../host/workspace";
import { sessionItem, sessionLook } from "../sessions/model";
import { currentHost } from "../services";
import { Button, StatusDot, T } from "../ui/components";
import { radius, space, useTheme } from "../ui/tokens";
import { useHostName } from "./HostFrame";

type Shown = { step: "confirm" } | Exclude<RemovalStep, { step: "removed" }>;

const plural = (n: number, one: string, many: string) => (n === 1 ? `1 ${one}` : `${n} ${many}`);

export function RemoveWorktree({ worktree, onDone, onCancel }: { worktree: WorkspaceInfo; onDone(): void; onCancel(): void }) {
  const theme = useTheme();
  const hostName = useHostName();
  const state = useHostState();
  const [shown, setShown] = useState<Shown>({ step: "confirm" });
  const [working, setWorking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const branch = `⎇ ${worktree.branch ?? workspaceLabel(worktree)}`;

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
  const remove = (force = false) => run(() => removeWorktree(currentHost, worktree.workspace_id, force), (e) => removalError(e, hostName));

  let body;
  switch (shown.step) {
    case "confirm":
      body = (
        <>
          <T variant="title">Remove {branch}?</T>
          <T color={theme.mut}>
            Its checkout is deleted from {hostName}. The branch keeps its commits, and its sessions stay readable.
          </T>
          {error ? <ErrorText text={error} /> : null}
          <Button tall kind="destructive" label="Remove worktree" loading={working} onPress={() => void remove()} testID="remove-confirm" />
        </>
      );
      break;
    case "busy": {
      const { session_ids } = shown.busy;
      const other = otherBlockers(shown.busy);
      const stop = () =>
        run(
          () => stopThenRemove(currentHost, worktree.workspace_id, session_ids),
          (e) => stopAgentError(e, hostName),
        );
      body = (
        <>
          <T variant="title">Still running in {branch}</T>
          <T color={theme.mut}>{hostName} won't remove a worktree while agents or terminals run there.</T>
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
              label={`Stop ${plural(session_ids.length, "agent", "agents")} and remove`}
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
          <T color={theme.mut}>
            {branch} has changes that aren't committed. Removing it anyway deletes them; the branch keeps its commits.
          </T>
          {error ? <ErrorText text={error} /> : null}
          <Button tall kind="destructive" label="Remove anyway" loading={working} onPress={() => void remove(true)} testID="remove-force" />
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
