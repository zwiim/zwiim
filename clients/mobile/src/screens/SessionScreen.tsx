// A session (DESIGN.md, Screen 5), pushed over the Host shell: its header
// (title; agent · workspace · the host's path), the transcript as the agent
// writes it, the Now bar on what the agent is doing, and the composer. The
// request it waits on is answered here as on the Sessions tab. Everything on
// screen is the host's: a message shows once the host echoes it, and Stop
// holds until the host ends the turn. The session strip under the header
// switches between the sessions of its worktree in place.

import { useEffect, useMemo, useRef, useState } from "react";
import { FlatList, Keyboard, KeyboardAvoidingView, Platform, Pressable, StyleSheet, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import type { NativeStackScreenProps } from "@react-navigation/native-stack";
import * as Haptics from "expo-haptics";
import { randomUUID } from "expo-crypto";
import { isClientError, type AgentRequest, type SessionView } from "@zwiim/sdk";
import { useCurrentHost, useHostState, useNow } from "../app/hooks";
import type { RootStack } from "../app/navigation";
import { connectionText } from "../host/status";
import { requestTitle, sessionItem, type SessionItem } from "../sessions/model";
import { sessionToOpen, worktreeSessions } from "../sessions/worktree";
import {
  clock,
  nowState,
  transcriptRows,
  turnKey,
  turnRunning,
  type ToolItem,
  type TranscriptRow,
} from "../sessions/transcript";
import { currentHost } from "../services";
import { Button, EmptyState, SkeletonRows, T } from "../ui/components";
import { ChevronLeft, Icon } from "../ui/icons";
import { Markdown } from "../ui/markdown";
import {
  Caret,
  Composer,
  ErrorRow,
  FooterRow,
  NowAction,
  NowBar,
  ThoughtRow,
  ToolGroup,
  ToolRow,
  ToolSheet,
  UserMessage,
} from "../ui/session";
import { space, useTheme } from "../ui/tokens";
import { useHostName } from "./HostFrame";
import { RequestCard, RequestSheet } from "./requests";
import { SessionStrip } from "./SessionStrip";

/** When this phone first saw each running turn; the protocol carries no times. */
const turnStarts = new Map<string, number>();

export function SessionScreen({ route, navigation }: NativeStackScreenProps<RootStack, "Session">) {
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  const { session_id, started } = route.params;
  const name = useHostName();
  const { view: connection, store } = useCurrentHost();
  const state = useHostState();
  const item = useMemo(
    () => sessionItem(state, session_id, currentHost.requestSeenAt),
    [state.workspaces, state.sessionsByWorkspace, state.sessions, session_id],
  );
  const worktree = useMemo(
    () => (item?.workspace_id ? worktreeSessions(state, item.workspace_id, currentHost.requestSeenAt) : null),
    [state.workspaces, state.sessionsByWorkspace, state.sessions, item?.workspace_id],
  );
  const view = state.sessions[session_id];
  // Just started here: the host lists it on its feed shortly. Once listed, a missing session is gone.
  const listed = useRef<string | null>(null);
  if (item) listed.current = session_id;
  const waiting = !item && started && listed.current !== session_id;
  const connected = connection?.status === "live";
  const back = () => (navigation.canGoBack() ? navigation.goBack() : navigation.navigate("Host"));

  // On screen: stream the session and load its transcript until it's left.
  useEffect(() => {
    if (!store) return;
    currentHost.showSession(session_id);
    return () => currentHost.hideSession(session_id);
  }, [store, session_id]);

  // Switching back to this worktree reopens this session.
  useEffect(() => {
    if (item?.workspace_id) currentHost.rememberSession(item.workspace_id, session_id);
  }, [item?.workspace_id, session_id]);

  // Tabs swap the session in place; a worktree with none opens New session there.
  const open = (id: string) => navigation.setParams({ session_id: id, started: undefined });
  const startIn = (workspace_id: string) => navigation.navigate("NewSession", { workspace_id });
  const toWorktree = (workspace_id: string) => {
    const there = worktreeSessions(state, workspace_id)?.sessions ?? [];
    const id = sessionToOpen(there, currentHost.lastSession(workspace_id));
    if (id) open(id);
    else startIn(workspace_id);
  };

  const rows = useMemo(() => transcriptRows(view), [view]);
  const text = connectionText(connection);

  let body;
  if (state.workspaces === null || waiting) body = <SkeletonRows />;
  else if (!item) {
    body = (
      <EmptyState
        glyph="✕"
        title="This session is gone"
        body={`${name} no longer lists it. It may have been deleted.`}
        action={<Button kind="secondary" label="Back to Sessions" onPress={back} />}
      />
    );
  } else body = <Live key={session_id} item={item} view={view} rows={rows} hostName={name} connected={connected} />;

  return (
    <View style={[styles.page, { backgroundColor: theme.bg, paddingTop: insets.top }]}>
      <View style={[styles.header, { borderBottomColor: theme.line }]}>
        <Pressable accessibilityRole="button" accessibilityLabel="Back" onPress={back} style={styles.back} testID="session-back">
          <Icon icon={ChevronLeft} color={theme.fg} size={24} />
        </Pressable>
        <View style={styles.headerText}>
          <T variant="headline" numberOfLines={1} testID="session-title">
            {item?.title ?? (waiting ? "Starting…" : "Session")}
          </T>
          <T variant="monoSm" color={theme.mut} numberOfLines={1} testID="session-subtitle">
            {[item?.agent, item?.workspace, `${name} ${text.pill}`].filter(Boolean).join(" · ")}
          </T>
        </View>
      </View>
      {worktree && item ? (
        <SessionStrip
          worktree={worktree}
          session_id={session_id}
          onOpen={open}
          onNew={() => startIn(worktree.workspace_id)}
          onWorktree={toWorktree}
        />
      ) : null}
      {body}
    </View>
  );
}

/** Keyboard up: the composer rides it, and the home indicator is covered. */
function useKeyboardShown(): boolean {
  const [shown, setShown] = useState(false);
  useEffect(() => {
    const show = Keyboard.addListener(Platform.OS === "ios" ? "keyboardWillShow" : "keyboardDidShow", () => setShown(true));
    const hide = Keyboard.addListener(Platform.OS === "ios" ? "keyboardWillHide" : "keyboardDidHide", () => setShown(false));
    return () => {
      show.remove();
      hide.remove();
    };
  }, []);
  return shown;
}

function sendError(e: unknown, hostName: string): string {
  if (isClientError(e, "turn_in_progress")) return "The agent is still working. Stop it or wait.";
  if (isClientError(e, "unknown_outcome")) return "Not sure that went through. Refreshed from your Mac.";
  if (isClientError(e, "network") || isClientError(e, "timeout")) return `Not connected to ${hostName}. Try again when it's back.`;
  return "Couldn't send that. Try again.";
}

function stopError(e: unknown, hostName: string): string {
  if (isClientError(e, "unknown_outcome")) return "Not sure that went through. Refreshed from your Mac.";
  if (isClientError(e, "network") || isClientError(e, "timeout")) return `Not connected to ${hostName}. Try again when it's back.`;
  return "Couldn't stop the agent. Try again.";
}

/** A user message with this id is on screen: the host took it. */
const echoed = (view: SessionView | undefined, id: string) =>
  view?.live.userMessage?.message_id === id || view?.settled?.userMessage?.message_id === id;

function Live({
  item,
  view,
  rows,
  hostName,
  connected,
}: {
  item: SessionItem;
  view: SessionView | undefined;
  rows: TranscriptRow[];
  hostName: string;
  connected: boolean;
}) {
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  const keyboard = useKeyboardShown();
  const { view: connection } = useCurrentHost();
  const session_id = item.session_id;
  const running = turnRunning(view);
  const now = nowState(view, connected);
  const request = view?.pendingRequest ?? null;
  const tick = useNow(running || now?.type === "reconnecting" || request !== null);

  // The running turn's timer, from when this phone first saw it.
  const key = turnKey(session_id, view);
  if (key && !turnStarts.has(key)) turnStarts.set(key, Date.now());
  const started = key ? turnStarts.get(key)! : null;

  const [draft, setDraft] = useState("");
  const [sending, setSending] = useState(false);
  const [unsure, setUnsure] = useState<string | null>(null);
  const [stopping, setStopping] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [tool, setTool] = useState<ToolItem | null>(null);
  const [reviewing, setReviewing] = useState(false);

  // Stop holds until the host ends the turn.
  useEffect(() => {
    if (!running) setStopping(false);
  }, [running]);

  // A send whose outcome was unknown: the host's echo settles it.
  useEffect(() => {
    if (unsure && echoed(view, unsure)) {
      setUnsure(null);
      setDraft("");
      setNotice(null);
    }
  }, [view, unsure]);

  // A request that arrives while the session is open presents itself, once.
  const seen = useRef(request?.request_id ?? null);
  useEffect(() => {
    if (!request || request.request_id === seen.current) return;
    seen.current = request.request_id;
    void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Warning);
    setReviewing(true);
  }, [request?.request_id]);
  useEffect(() => {
    if (!request) setReviewing(false);
  }, [request]);

  const send = async () => {
    const text = draft.trim();
    if (!text || sending) return;
    const id = randomUUID();
    setSending(true);
    setNotice(null);
    setUnsure(null);
    void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Light);
    try {
      await currentHost.send(session_id, text, id);
      setDraft("");
    } catch (e) {
      if (isClientError(e, "unknown_outcome")) setUnsure(id);
      setNotice(sendError(e, hostName));
    } finally {
      setSending(false);
    }
  };

  const stop = async () => {
    setStopping(true);
    setNotice(null);
    try {
      await currentHost.cancel(session_id);
    } catch (e) {
      // The turn may have ended on its own meanwhile.
      if (!isClientError(e, "session_not_live")) setNotice(stopError(e, hostName));
      setStopping(false);
    }
  };

  const loading = view?.history == null && rows.length === 0 && view?.attach !== "error";
  const attachError = view?.attach === "error" ? view.attachError?.message ?? "Couldn't open this session." : null;
  const empty = !loading && rows.length === 0 && !request && !attachError;
  const agentAway = view?.attach === "detached" || view?.agentStatus === "disconnected";

  const renderRow = ({ item: row, index }: { item: TranscriptRow; index: number }) => {
    // Inverted: index 0 is the newest, and `newer` the row below this one.
    const newer = index > 0 ? rows[rows.length - index] : undefined;
    const gap = newer && (newer.type === "user" || row.type === "user") ? space.lg : space.sm;
    return (
      <View style={{ marginTop: index === rows.length - 1 ? space.md : 0, marginBottom: index === 0 ? space.md : gap }}>
        <Row row={row} onTool={setTool} />
      </View>
    );
  };

  const nowBar = (() => {
    if (!now) return null;
    const stopButton = running ? (
      <NowAction kind="inverse" label="Stop" onPress={() => void stop()} loading={stopping} disabled={!connected} testID="now-stop" />
    ) : null;
    const time = started ? clock(tick - started) : null;
    switch (now.type) {
      case "working":
        return <NowBar tone="plain" dot="work" text={now.text} time={time} action={stopButton} />;
      case "thinking":
        return <NowBar tone="plain" dot="work" text="Thinking" time={time} action={stopButton} />;
      case "needs_you":
        return (
          <NowBar
            tone="need"
            dot="need"
            text={`Waiting for you · ${request ? requestTitle(request) : "a request"}`}
            action={<NowAction kind="need" label="Review" onPress={() => setReviewing(true)} testID="now-review" />}
          />
        );
      case "reconnecting": {
        const wait = connection?.retryAt ? Math.ceil((connection.retryAt - tick) / 1000) : 0;
        return <NowBar tone="plain" dot="work" text={wait > 0 ? `Reconnecting… retry in ${wait}s` : "Reconnecting…"} />;
      }
      case "disconnected":
        return <NowBar tone="plain" dot="off" text="Agent disconnected" />;
    }
  })();

  return (
    <KeyboardAvoidingView style={styles.flex} behavior={Platform.OS === "ios" ? "padding" : "height"}>
      {/* Reconnecting: the last-known transcript, dimmed; it still scrolls. */}
      <View style={[styles.flex, !connected && styles.dim]}>
        {loading ? (
          <SkeletonRows />
        ) : empty ? (
          <EmptyState glyph="≡" title="No messages yet" body={`Send a message to start the agent on ${hostName}.`} />
        ) : (
          <FlatList
            inverted
            data={[...rows].reverse()}
            keyExtractor={(r) => r.key}
            renderItem={renderRow}
            contentContainerStyle={styles.list}
            keyboardShouldPersistTaps="handled"
            keyboardDismissMode="interactive"
            ListHeaderComponent={
              <View pointerEvents={connected ? "auto" : "none"} style={styles.headerStack}>
                {attachError ? <ErrorRow message={attachError} /> : null}
                {request ? (
                  <RequestCard
                    item={item as SessionItem & { request: AgentRequest }}
                    hostName={hostName}
                    now={tick}
                    showTitle={false}
                  />
                ) : null}
              </View>
            }
            testID="transcript"
          />
        )}
      </View>
      <View
        style={{ paddingBottom: keyboard ? space.sm : Math.max(insets.bottom, space.sm) }}
        pointerEvents={connected ? "auto" : "none"}
      >
        {nowBar}
        {notice ? (
          <T variant="callout" color={theme.err} style={styles.notice} testID="session-notice">
            {notice}
          </T>
        ) : null}
        <Composer
          value={draft}
          onChange={(t) => {
            setDraft(t);
            if (unsure) setUnsure(null);
          }}
          onSend={() => void send()}
          placeholder={agentAway ? "Send to restart the agent" : "Reply or steer…"}
          disabled={!connected}
          sending={sending}
        />
      </View>
      <ToolSheet tool={tool} onClose={() => setTool(null)} />
      {request ? (
        <RequestSheet
          visible={reviewing && connected}
          onClose={() => setReviewing(false)}
          session_id={session_id}
          request={request}
          hostName={hostName}
        />
      ) : null}
    </KeyboardAvoidingView>
  );
}

function Row({ row, onTool }: { row: TranscriptRow; onTool(tool: ToolItem): void }) {
  switch (row.type) {
    case "user":
      return <UserMessage text={row.text} />;
    case "agent":
      return <Markdown text={row.text} caret={row.streaming ? <Caret /> : undefined} />;
    case "thought":
      return <ThoughtRow text={row.text} streaming={row.streaming} />;
    case "tool":
      return <ToolRow tool={row.tool} onPress={() => onTool(row.tool)} />;
    case "tools":
      return <ToolGroup tools={row.tools} onOpen={onTool} />;
    case "error":
      return <ErrorRow message={row.message} />;
    case "footer":
      return <FooterRow text={row.text} stopped={row.stopped} />;
  }
}

const styles = StyleSheet.create({
  page: { flex: 1 },
  flex: { flex: 1 },
  header: {
    minHeight: 56,
    flexDirection: "row",
    alignItems: "center",
    paddingRight: space.lg,
    borderBottomWidth: StyleSheet.hairlineWidth,
  },
  back: { width: 44, height: 44, alignItems: "center", justifyContent: "center" },
  headerText: { flex: 1, gap: space.xxs },
  dim: { opacity: 0.6 },
  list: { paddingHorizontal: space.lg },
  headerStack: { gap: space.sm, paddingBottom: space.md },
  notice: { paddingHorizontal: space.lg, paddingTop: space.sm },
});
