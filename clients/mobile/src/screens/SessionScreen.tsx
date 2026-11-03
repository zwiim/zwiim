// A session (DESIGN.md, Screen 5), pushed over the Host shell: its header
// (title; agent · workspace · the host's path), the transcript as the agent
// writes it, the Now bar on what the agent is doing, and the composer. The
// request it waits on is answered here as on the Sessions tab. Everything on
// screen is the host's: a message shows once the host echoes it, and Stop
// holds until the host ends the turn. Stop agent (in the header) ends the
// agent and its background work, keeping the session. The session strip under
// the header switches between the sessions of its worktree in place, and the
// PR strip under it follows the worktree branch's pull request. Chips over
// the composer (and the header's `⋯`) open Session options: the model, effort
// and mode the Host reports, changed between turns, and the agent's context.

import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type Dispatch, type ReactNode, type SetStateAction } from "react";
import { FlatList, Keyboard, KeyboardAvoidingView, Platform, Pressable, StyleSheet, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import type { NativeStackScreenProps } from "@react-navigation/native-stack";
import * as Haptics from "expo-haptics";
import { randomUUID } from "expo-crypto";
import {
  ATTACHMENT_GONE_MESSAGE,
  conflictView,
  derivePlanUi,
  hasCapability,
  isClientError,
  meterTakesNowBar,
  offersChatRewind,
  revertNotes,
  workspaceFile,
  type AgentRequest,
  type AgentType,
  type AttachmentBlock,
  type PlanContinues,
  type PlanUi,
  type SavedPlan,
  type RevertChoice,
  type SessionView,
  waitingText,
} from "@zwiim/sdk";
import { pickFiles, pickPhotos } from "../attachments/native";
import { UPLOAD_FAILED_MESSAGE } from "../attachments/pending";
import { usePendingAttachments } from "../attachments/usePendingAttachments";
import { fileScreen, openTarget } from "../preview/model";
import { useCurrentHost, useHostState, useNow } from "../app/hooks";
import { useRootNavigation, type RootStack } from "../app/navigation";
import { connectionText } from "../host/status";
import { requestTitle, sessionItem, type SessionItem } from "../sessions/model";
import { sessionToOpen, worktreeSessions } from "../sessions/worktree";
import {
  clock,
  nowState,
  transcriptBuilder,
  turnKey,
  turnRunning,
  type ToolItem,
  type RevertPoint,
  type TranscriptRow,
} from "../sessions/transcript";
import {
  continueBlock,
  continueLabel,
  doneCount,
  listedTitle,
  nowActivity,
  pipStates,
  planNowState,
  planSlot,
  runContinue,
  stoppedTitle,
} from "../sessions/plan";
import { canReset, oneReset, revertError } from "../sessions/revert";
import { offeredAgents } from "../sessions/start";
import { stopAgentError } from "../host/removal";
import { agentName, lockedNote } from "../sessions/controls";
import { currentHost } from "../services";
import { Button, EmptyState, Sheet, SkeletonRows, T } from "../ui/components";
import { ChevronLeft, Ellipsis, Icon } from "../ui/icons";
import { Markdown } from "../ui/markdown";
import {
  AgentImageChip,
  AttachSheet,
  Caret,
  Composer,
  ContinuedCard,
  ContinuedInRow,
  ErrorRow,
  UnreachableCard,
  FooterRow,
  NowAction,
  NowBar,
  ResumePanel,
  RevertAction,
  RevertSheet,
  StepMeter,
  StepSheet,
  SubagentRow,
  ThoughtRow,
  ToolGroup,
  ToolRow,
  ToolSheet,
  UserMessage,
} from "../ui/session";
import { space, useTheme } from "../ui/tokens";
import { useHostName } from "./HostFrame";
import { RequestCard, RequestSheet } from "./requests";
import { ComposerChips, SessionOptionsSheet } from "./SessionControls";
import { PrStrip } from "./PrStrip";
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
  const [stopAgent, setStopAgent] = useState(false);
  const [optionsOpen, setOptionsOpen] = useState(false);
  const agent = agentName(state.agents, item?.agent);
  const back = () => (navigation.canGoBack() ? navigation.goBack() : navigation.navigate("Host"));
  // An edited file opens on its own screen while its worktree is still there.
  const workspace = state.workspaces?.find((w) => w.workspace_id === item?.workspace_id && !w.removed) ?? null;
  // Continue is refused in the repository folder and in an archived worktree, which `workspace` leaves out.
  const blocked = continueBlock(state.workspaces?.find((w) => w.workspace_id === item?.workspace_id));
  const fileOf = useCallback((path: string) => (workspace ? workspaceFile(path, workspace) : null), [workspace]);
  const openFile = useCallback(
    (path: string) => {
      if (!workspace) return;
      const { workspace_id } = workspace;
      navigation.push(fileScreen(path, workspace.is_git_repo), { workspace_id, path });
    },
    [workspace, navigation],
  );

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
  const open = (id: string) => {
    setStopAgent(false);
    setOptionsOpen(false);
    navigation.setParams({ session_id: id, started: undefined });
  };
  const startIn = (workspace_id: string) => navigation.navigate("NewSession", { workspace_id });
  const toWorktree = (workspace_id: string) => {
    const there = worktreeSessions(state, workspace_id)?.sessions ?? [];
    const id = sessionToOpen(there, currentHost.lastSession(workspace_id));
    if (id) open(id);
    else startIn(workspace_id);
  };

  // The history's rows stay the same objects while only the live turn changes.
  const [build] = useState(transcriptBuilder);
  const canRetry = hasCapability(connection?.compat ?? null, "session.retry");
  const rows = useMemo(() => build(view, canRetry), [build, view, canRetry]);
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
  } else {
    body = (
      <Live
        key={session_id}
        item={item}
        view={view}
        rows={rows}
        hostName={name}
        agentName={agent ?? "Agent"}
        connected={connected}
        onOptions={() => setOptionsOpen(true)}
        fileOf={fileOf}
        onOpenFile={openFile}
        resettable={canReset(workspace)}
        continueBlocked={blocked}
      />
    );
  }

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
            {[agent, item?.workspace, `${name} ${text.pill}`].filter(Boolean).join(" · ")}
          </T>
        </View>
        {item?.live ? (
          <Button
            kind="destructive"
            label="Stop agent"
            disabled={!connected}
            onPress={() => setStopAgent(true)}
            testID="session-stop-agent"
          />
        ) : null}
        {item ? (
          <Pressable
            accessibilityRole="button"
            accessibilityLabel="Session options"
            onPress={() => setOptionsOpen(true)}
            style={styles.more}
            testID="session-options"
          >
            <Icon icon={Ellipsis} color={theme.fg} size={22} />
          </Pressable>
        ) : null}
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
      {worktree && item ? (
        <PrStrip
          workspace_id={worktree.workspace_id}
          entry={state.prs[worktree.workspace_id]}
          workspace={state.workspaces?.find((w) => w.workspace_id === worktree.workspace_id)}
          compat={connection?.compat ?? null}
          conflicts={conflictView(state, worktree.workspace_id, session_id)}
          connected={connected}
          hostName={name}
        />
      ) : null}
      {body}
      <StopAgentSheet
        key={session_id}
        visible={stopAgent && !!item?.live}
        session_id={session_id}
        hostName={name}
        onClose={() => setStopAgent(false)}
      />
      <SessionOptionsSheet
        key={`options-${session_id}`}
        visible={optionsOpen && !!item}
        onClose={() => setOptionsOpen(false)}
        session_id={session_id}
        agent={agent}
        view={view}
        connected={connected}
        running={turnRunning(view)}
        hostName={name}
      />
    </View>
  );
}

/**
 * Stop the session's agent, not just its turn: the agent and whatever it runs
 * in the background end on the host; the session and its transcript stay.
 * Closes once the host has stopped it; never retried.
 */
function StopAgentSheet({
  visible,
  session_id,
  hostName,
  onClose,
}: {
  visible: boolean;
  session_id: string;
  hostName: string;
  onClose(): void;
}) {
  const theme = useTheme();
  const [stopping, setStopping] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const close = () => {
    setError(null);
    onClose();
  };
  const stop = async () => {
    setStopping(true);
    setError(null);
    try {
      await currentHost.stopSession(session_id);
      close();
    } catch (e) {
      setError(stopAgentError(e, hostName));
    } finally {
      setStopping(false);
    }
  };
  return (
    <Sheet visible={visible} onClose={close}>
      <T variant="title">Stop the agent?</T>
      <T color={theme.mut} testID="stop-agent-body">
        The agent stops on {hostName}, and so does any work it left running in the background. This session and its
        transcript stay. Your next message starts the agent again.
      </T>
      {error ? (
        <T color={theme.err} testID="stop-agent-error">
          {error}
        </T>
      ) : null}
      <Button tall kind="destructive" label="Stop agent" loading={stopping} onPress={() => void stop()} testID="stop-agent-confirm" />
      <Button tall kind="secondary" label="Cancel" onPress={close} />
    </Sheet>
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
  if (isClientError(e, "attachment_not_found")) return ATTACHMENT_GONE_MESSAGE;
  if (isClientError(e, "turn_in_progress")) return "The agent is still working. Stop it or wait.";
  if (isClientError(e, "unknown_outcome")) return "Not sure that went through. Refreshed from your Mac.";
  if (isClientError(e, "network") || isClientError(e, "timeout")) return `Not connected to ${hostName}. Try again when it's back.`;
  return "Couldn't send that. Try again.";
}

/** A clock for the parts that show it, so a ticking second doesn't redraw the transcript. */
function Ticking({ on, children }: { on: boolean; children(now: number): ReactNode }) {
  const now = useNow(on);
  return <>{children(now)}</>;
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
  agentName,
  connected,
  onOptions,
  fileOf,
  onOpenFile,
  resettable,
  continueBlocked,
}: {
  item: SessionItem;
  view: SessionView | undefined;
  rows: TranscriptRow[];
  hostName: string;
  agentName: string;
  connected: boolean;
  onOptions(): void;
  fileOf(path: string): string | null;
  onOpenFile(path: string): void;
  resettable: boolean;
  /** Why Continue cannot run in this session's workspace; null when it can. */
  continueBlocked: string | null;
}) {
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  const keyboard = useKeyboardShown();
  const nav = useRootNavigation();
  const { view: connection } = useCurrentHost();
  const session_id = item.session_id;
  const running = turnRunning(view);
  const now = nowState(view, connected);
  const request = view?.pendingRequest ?? null;
  // The plan's meter, resume panel and links; nothing of it on a Host without progress.
  const showsPlan = hasCapability(connection?.compat ?? null, "agent.progress");
  const canContinue = hasCapability(connection?.compat ?? null, "session.continue_plan");
  const ui: PlanUi = showsPlan && view ? derivePlanUi(view) : { kind: "none" };
  const [stepsOpen, setStepsOpen] = useState(false);
  // A plan the Host said there is nothing to continue for, until history says otherwise.
  const [gonePlan, setGonePlan] = useState<string | null>(null);
  const continues = showsPlan ? view?.continues ?? null : null;
  const firstUserKey = useMemo(() => (continues ? rows.find((r) => r.type === "user")?.key ?? null : null), [continues, rows]);
  const cardShown = useMemo(() => rows.some((r) => r.type === "unreachable"), [rows]);
  const ticking = running || now?.type === "reconnecting" || now?.type === "waiting" || request !== null;

  // The running turn's timer, from when this phone first saw it.
  const key = turnKey(session_id, view);
  if (key && !turnStarts.has(key)) turnStarts.set(key, Date.now());
  const started = key ? turnStarts.get(key)! : null;

  // Kept per session by the host's controller, so switching tabs never loses it.
  const [draft, setDraftState] = useState(() => currentHost.draft(session_id));
  const setDraft = (text: string) => {
    currentHost.setDraft(session_id, text);
    setDraftState(text);
  };
  const [sending, setSending] = useState(false);
  const [unsure, setUnsure] = useState<string | null>(null);
  const [stopping, setStopping] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [tool, setTool] = useState<ToolItem | null>(null);
  const [reviewing, setReviewing] = useState(false);
  const [attaching, setAttaching] = useState(false);
  // The message a reset is being asked about, and what the last one left to say.
  const [resetting, setResetting] = useState<{ point: RevertPoint; chat: boolean } | null>(null);
  const [resetBusy, setResetBusy] = useState(false);
  const [resetError, setResetError] = useState<string | null>(null);
  const [resetNotes, setResetNotes] = useState<string[]>([]);
  const [runReset] = useState(() => oneReset<void>());
  const attachments = usePendingAttachments(hostName);
  const workspace_id = item.workspace_id;
  const canAttach = attachments.enabled && workspace_id !== null;

  // Stop holds until the host ends the turn.
  useEffect(() => {
    if (!running) setStopping(false);
  }, [running]);

  // A send whose outcome was unknown: the host's echo settles it.
  useEffect(() => {
    if (unsure && echoed(view, unsure)) {
      setUnsure(null);
      setDraft("");
      attachments.clear();
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
    const withFiles = canAttach && attachments.files.length > 0;
    if ((!text && !withFiles) || sending) return;
    const id = randomUUID();
    setSending(true);
    setNotice(null);
    setResetNotes([]);
    setUnsure(null);
    void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Light);
    try {
      // Files first, into this session's worktree; the message names them.
      const files = withFiles ? await attachments.upload(workspace_id!) : [];
      if (files === null) {
        setNotice(UPLOAD_FAILED_MESSAGE);
        return;
      }
      await currentHost.send(session_id, text, id, files);
      setDraftState(currentHost.draft(session_id));
      attachments.clear();
    } catch (e) {
      if (isClientError(e, "unknown_outcome")) setUnsure(id);
      if (isClientError(e, "attachment_not_found")) attachments.forgetUploads();
      setNotice(sendError(e, hostName));
    } finally {
      setSending(false);
    }
  };

  // The picker opens once the attach sheet has gone; iOS shows one modal at a time.
  const picking = useRef<"photos" | "files" | null>(null);
  const pick = (from: "photos" | "files") => {
    picking.current = from;
    setAttaching(false);
  };
  const openPicker = () => {
    const from = picking.current;
    picking.current = null;
    if (!from) return;
    (from === "photos" ? pickPhotos() : pickFiles())
      .then(attachments.add)
      .catch(() => setNotice(from === "photos" ? "Couldn't open your photos." : "Couldn't open Files."));
  };

  // A sent file opens in-app: text in the file view, anything else in Preview.
  const openSent = (file: AttachmentBlock) => {
    if (!workspace_id) return;
    setNotice(null);
    nav.navigate(openTarget(file.path), { workspace_id, path: file.path });
  };

  // An image the agent produced, or a file its message links to, opens the same way.
  const openImage = (path: string) => {
    if (!workspace_id) return;
    setNotice(null);
    nav.navigate(openTarget(path), { workspace_id, path });
  };

  const askReset = (point: RevertPoint) => {
    setResetError(null);
    setResetting({ point, chat: offersChatRewind(point.before, view?.options, connection?.compat ?? null) });
  };
  const closeReset = () => {
    if (resetBusy) return;
    setResetting(null);
    setResetError(null);
  };
  // A chat taken back arrives as the session's next snapshot; nothing to refetch.
  const reset = (choice: RevertChoice) => {
    if (!resetting) return;
    const { turn_id } = resetting.point;
    return runReset(async () => {
      setResetBusy(true);
      setResetError(null);
      setResetNotes([]);
      try {
        const result = await currentHost.revertSession(session_id, turn_id, choice);
        setResetNotes(revertNotes(result, choice));
        setResetting(null);
      } catch (e) {
        setResetError(revertError(e, hostName));
      } finally {
        setResetBusy(false);
      }
    });
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

  // An edit to one file in the worktree opens that file; any other tool call its details.
  const toolFile = useCallback((tool: ToolItem) => (tool.path ? fileOf(tool.path) : null), [fileOf]);
  const onTool = (tool: ToolItem) => {
    const path = toolFile(tool);
    if (path) onOpenFile(path);
    else setTool(tool);
  };

  // The list redraws a row only when its row, its place or `renderRow` changes,
  // so `renderRow` reads what changes every render through these refs.
  // The rows are set while rendering, not in an effect: the list draws its
  // cells in this same render, before any effect runs, and a cell drawn from
  // the previous rows would keep the wrong gap (nothing redraws it after). A
  // render React throws away leaves rows the next render replaces before any
  // cell reads them.
  const shownRows = useRef(rows);
  shownRows.current = rows;
  // Send the failed turn again, or carry it into a new chat and open that.
  const retryTurn = () => currentHost.retrySession(session_id);
  const retryInNewChat = () => currentHost.retryInNewChat(session_id);
  const openStarted = (id: string) => nav.replace("Session", { session_id: id, started: true });
  // A session this plan came from or went to opens over this one, so Back returns here.
  const openLinked = (id: string) => nav.push("Session", { session_id: id });
  // The handlers are only called on a press, after a commit.
  const latest = useRef({ onTool, openSent, openImage, askReset, retryTurn, retryInNewChat, openStarted, openLinked });
  useLayoutEffect(() => {
    latest.current = { onTool, openSent, openImage, askReset, retryTurn, retryInNewChat, openStarted, openLinked };
  });
  const [actions] = useState(() => ({
    onTool: (tool: ToolItem) => latest.current.onTool(tool),
    openSent: (file: AttachmentBlock) => latest.current.openSent(file),
    openImage: (path: string) => latest.current.openImage(path),
    askReset: (point: RevertPoint) => latest.current.askReset(point),
    retryTurn: () => latest.current.retryTurn(),
    retryInNewChat: () => latest.current.retryInNewChat(),
    openStarted: (id: string) => latest.current.openStarted(id),
    openLinked: (id: string) => latest.current.openLinked(id),
  }));
  const hasWorkspace = workspace_id !== null;
  // Each cell is memoized on its row and index, and its gap depends on the row
  // below it. That holds because a change to a row's neighbour always changes
  // the row's index or identity: rows are added only at the newest end (every
  // index shifts), live rows are rebuilt on each change, and anything that
  // changes the history's rows rebuilds them all. Keep it so, or pass the
  // neighbour in.
  const renderRow = useCallback(
    ({ item: row, index }: { item: TranscriptRow; index: number }) => {
      const rows = shownRows.current;
      // Inverted: index 0 is the newest, and `newer` the row below this one.
      const newer = index > 0 ? rows[rows.length - index] : undefined;
      const gap = newer && (newer.type === "user" || row.type === "user") ? space.lg : space.sm;
      return (
        <View style={{ marginTop: index === rows.length - 1 ? space.md : 0, marginBottom: index === 0 ? space.md : gap }}>
          <Row
            row={row}
            onTool={actions.onTool}
            toolFile={toolFile}
            fileOf={fileOf}
            onOpenFile={onOpenFile}
            onOpenSent={hasWorkspace ? actions.openSent : undefined}
            onOpenImage={hasWorkspace ? actions.openImage : undefined}
            onRevert={resettable ? actions.askReset : undefined}
            revertDisabled={running || !connected}
            continued={continues && row.key === firstUserKey ? { continues, onOpenSession: actions.openLinked } : undefined}
            unreachable={{
              agentName,
              online: connected,
              hostName,
              onRetry: actions.retryTurn,
              onRetryInNewChat: actions.retryInNewChat,
              onStarted: actions.openStarted,
            }}
          />
        </View>
      );
    },
    [actions, toolFile, fileOf, onOpenFile, hasWorkspace, resettable, running, connected, agentName, hostName, continues, firstUserKey],
  );

  const nowBar = (tick: number) => {
    if (!now) return null;
    const stopButton = running ? (
      <NowAction kind="inverse" label="Stop" onPress={() => void stop()} loading={stopping} disabled={!connected} testID="now-stop" />
    ) : null;
    const time = started ? clock(tick - started) : null;
    if (ui.kind === "working" && meterTakesNowBar(ui, planNowState(now))) {
      const step = ui.current === null ? undefined : ui.progress.steps[ui.current];
      return (
        <StepMeter
          title={step?.title ?? "Working"}
          activity={nowActivity(now)}
          count={`${ui.progress.completed} of ${ui.progress.total}`}
          time={time}
          states={pipStates(ui)}
          action={stopButton}
          onOpen={() => setStepsOpen(true)}
        />
      );
    }
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
      case "waiting":
        return <NowBar tone="plain" dot="work" text={waitingText(now.count)} time={clock(tick - now.since)} />;
    }
  };
  const newestFirst = useMemo(() => [...rows].reverse(), [rows]);
  useEffect(() => {
    if (ui.kind !== "working") setStepsOpen(false);
  }, [ui.kind]);

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
            data={newestFirst}
            keyExtractor={rowKey}
            renderItem={renderRow}
            windowSize={11}
            maxToRenderPerBatch={8}
            // On iOS, clipping an inverted list can leave it blank.
            removeClippedSubviews={Platform.OS === "android"}
            contentContainerStyle={styles.list}
            keyboardShouldPersistTaps="handled"
            keyboardDismissMode="interactive"
            ListHeaderComponent={
              <View pointerEvents={connected ? "auto" : "none"} style={styles.headerStack}>
                {attachError ? <ErrorRow message={attachError} /> : null}
                {request ? (
                  <Ticking on>
                    {(tick) => (
                      <RequestCard
                        item={item as SessionItem & { request: AgentRequest }}
                        hostName={hostName}
                        now={tick}
                        showTitle={false}
                      />
                    )}
                  </Ticking>
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
        <Ticking on={ticking}>{nowBar}</Ticking>
        {!now && view ? (
          <PlanSlot
            view={view}
            ui={ui}
            showsPlan={showsPlan}
            own={item.agent}
            canContinue={canContinue}
            blocked={continueBlocked}
            cardShown={cardShown}
            connected={connected}
            hostName={hostName}
            gone={gonePlan}
            onGone={setGonePlan}
            onOpenSession={actions.openLinked}
            onStarted={actions.openStarted}
            onNotice={setNotice}
          />
        ) : null}
        {notice ? (
          <T variant="callout" color={theme.err} style={styles.notice} testID="session-notice">
            {notice}
          </T>
        ) : null}
        {resetNotes.map((note) => (
          <T key={note} variant="callout" color={theme.mut} style={styles.notice} testID="session-revert-note">
            {note}
          </T>
        ))}
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
          controls={<ComposerChips view={view} locked={running} onOpen={onOptions} />}
          note={lockedNote(view?.options ?? null, running)}
          border={view?.options?.mode?.current === "unattended" ? theme.need : undefined}
          files={canAttach ? attachments.files : []}
          fileErrors={canAttach ? attachments.errors : []}
          onAttach={canAttach ? () => setAttaching(true) : undefined}
          onRemoveFile={attachments.remove}
        />
      </View>
      <AttachSheet visible={attaching} onClose={() => setAttaching(false)} onPick={pick} onClosed={openPicker} />
      <ToolSheet tool={tool} onClose={() => setTool(null)} />
      {ui.kind === "working" ? (
        <StepSheet
          visible={stepsOpen}
          onClose={() => setStepsOpen(false)}
          count={`${ui.progress.completed} of ${ui.progress.total}`}
          steps={ui.progress.steps}
          states={pipStates(ui)}
          activity={nowActivity(now)}
        />
      ) : null}
      <RevertSheet
        visible={resetting !== null}
        chat={resetting?.chat ?? false}
        disabled={!connected || running}
        error={resetError}
        resetting={resetBusy}
        onReset={(choice) => void reset(choice)}
        onClose={closeReset}
      />
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

const rowKey = (row: TranscriptRow) => row.key;

/**
 * The first message of a session that finishes another's plan, as a card. It
 * links back to the old session only while the store still lists it; the
 * message keeps its own "Reset to here".
 */
function ContinuedFirst({
  continues,
  files,
  onOpenFile,
  onOpenSession,
  onRevert,
  revertDisabled,
}: {
  continues: PlanContinues;
  files: AttachmentBlock[];
  onOpenFile?(file: AttachmentBlock): void;
  onOpenSession(id: string): void;
  onRevert?(): void;
  revertDisabled: boolean;
}) {
  const state = useHostState();
  const listed = listedTitle(state.sessionsByWorkspace, continues.session_id);
  return (
    <ContinuedCard
      agentName={continues.agent ? agentName(state.agents, continues.agent) : null}
      completed={continues.completed}
      total={continues.total}
      files={files}
      from={continues.title || listed || null}
      onOpenFile={onOpenFile}
      onOpenFrom={listed !== undefined ? () => onOpenSession(continues.session_id) : undefined}
      revert={onRevert ? <RevertAction onPress={onRevert} disabled={revertDisabled} /> : undefined}
    />
  );
}

/**
 * Over the composer while the agent is idle: where the plan went, or the
 * resume panel when it stopped part way. Nothing once the plan is done.
 */
function PlanSlot({
  view,
  ui,
  showsPlan,
  own,
  canContinue,
  blocked,
  cardShown,
  connected,
  hostName,
  gone,
  onGone,
  onOpenSession,
  onStarted,
  onNotice,
}: {
  view: SessionView;
  ui: PlanUi;
  showsPlan: boolean;
  own: string | null;
  canContinue: boolean;
  blocked: string | null;
  /** The can't-reach card shows: it is the only card, so there is no resume panel. */
  cardShown: boolean;
  connected: boolean;
  hostName: string;
  gone: string | null;
  onGone(plan_id: string): void;
  onOpenSession(id: string): void;
  onStarted(id: string): void;
  /** Where a failed Continue is said: the screen's notice above the composer. */
  onNotice: Dispatch<SetStateAction<string | null>>;
}) {
  const state = useHostState();
  const slot = planSlot({ ui, showsPlan, canContinue, cardShown, gone });
  if (slot === "continued" && ui.kind === "continued") {
    const id = ui.sessionId;
    return <ContinuedInRow title={listedTitle(state.sessionsByWorkspace, id)} onOpen={() => onOpenSession(id)} />;
  }
  if (slot !== "resume" || ui.kind !== "stopped") return null;
  return (
    <ResumeSlot
      key={ui.plan.plan_id}
      session_id={view.session_id}
      plan={ui.plan}
      stoppedAt={ui.stoppedAt}
      own={(own ?? ui.plan.agent) as AgentType}
      blocked={blocked}
      connected={connected}
      hostName={hostName}
      onGone={() => onGone(ui.plan.plan_id)}
      onStarted={onStarted}
      onNotice={onNotice}
    />
  );
}

function ResumeSlot({
  session_id,
  plan,
  stoppedAt,
  own,
  blocked,
  connected,
  hostName,
  onGone,
  onStarted,
  onNotice,
}: {
  session_id: string;
  plan: SavedPlan;
  stoppedAt: number;
  own: AgentType;
  blocked: string | null;
  connected: boolean;
  hostName: string;
  onGone(): void;
  onStarted(id: string): void;
  onNotice: Dispatch<SetStateAction<string | null>>;
}) {
  const state = useHostState();
  // The session's own agent first and chosen; the others the new-session list offers and can run.
  const choices = [
    { agent: own, name: agentName(state.agents, own) ?? own },
    ...(offeredAgents(state.agents) ?? [])
      .filter((a) => a.agent !== own && a.available)
      .map((a) => ({ agent: a.agent, name: a.name })),
  ];
  const [picked, setPicked] = useState<AgentType>(own);
  const [pending, setPending] = useState(false);
  // The notice this Continue set, if it is still showing: it goes with the panel (or the next tap), others stay.
  const said = useRef<string | null>(null);
  const say = (text: string | null) => {
    const mine = said.current;
    said.current = text;
    onNotice((cur) => (text ?? (cur === mine ? null : cur)));
  };
  const sayRef = useRef(say);
  useEffect(() => {
    sayRef.current = say;
  });
  useEffect(() => () => sayRef.current(null), []);
  const run = async () => {
    const id = await runContinue(currentHost, { session_id, agent: own }, picked, { pending: setPending, error: say, gone: onGone }, hostName);
    // Outside the op: a throwing callback is not the Host refusing.
    if (id) onStarted(id);
  };
  return (
    <ResumePanel
      heading={stoppedTitle(stoppedAt + 1, plan.steps[stoppedAt]?.title)}
      count={doneCount(plan.completed, plan.total)}
      steps={plan.steps}
      states={pipStates({ kind: "stopped", plan, stoppedAt })}
      choices={choices}
      picked={picked}
      onPick={setPicked}
      label={continueLabel(picked === own, choices.find((c) => c.agent === picked)?.name ?? picked)}
      pending={pending}
      disabled={!connected || blocked !== null}
      reason={blocked}
      onContinue={() => void run()}
    />
  );
}

function Row({
  row,
  onTool,
  toolFile,
  fileOf,
  onOpenFile,
  onOpenSent,
  onOpenImage,
  onRevert,
  revertDisabled,
  continued,
  unreachable,
}: {
  row: TranscriptRow;
  onTool(tool: ToolItem): void;
  toolFile(tool: ToolItem): string | null;
  fileOf(path: string): string | null;
  onOpenFile(path: string): void;
  onOpenSent?(file: AttachmentBlock): void;
  onOpenImage?(path: string): void;
  onRevert?(point: RevertPoint): void;
  revertDisabled: boolean;
  /** Set on the first message of a session that continues a plan: it shows as a card. */
  continued?: { continues: PlanContinues; onOpenSession(id: string): void };
  /** What the can't-reach card needs; only the transcript's top level has one. */
  unreachable?: {
    agentName: string;
    online: boolean;
    hostName: string;
    onRetry(): Promise<void>;
    onRetryInNewChat(): Promise<string>;
    onStarted(session_id: string): void;
  };
}) {
  const opens = (tool: ToolItem) => toolFile(tool) !== null;
  switch (row.type) {
    case "user": {
      const point = row.revert;
      if (continued) {
        return (
          <ContinuedFirst
            continues={continued.continues}
            files={row.files}
            onOpenFile={onOpenSent}
            onOpenSession={continued.onOpenSession}
            onRevert={point && onRevert ? () => onRevert(point) : undefined}
            revertDisabled={revertDisabled}
          />
        );
      }
      return (
        <UserMessage
          text={row.text}
          files={row.files}
          onOpenFile={onOpenSent}
          onRevert={point && onRevert ? () => onRevert(point) : undefined}
          revertDisabled={revertDisabled}
        />
      );
    }
    case "agent":
      return <Markdown text={row.text} caret={row.streaming ? <Caret /> : undefined} fileOf={fileOf} onOpen={onOpenImage} />;
    case "thought":
      return <ThoughtRow text={row.text} streaming={row.streaming} />;
    case "tool":
      return <ToolRow tool={row.tool} onPress={() => onTool(row.tool)} opens={opens(row.tool)} />;
    case "tools":
      return (
        <ToolGroup
          rows={row.rows}
          renderRow={(child) => (
            <Row
              row={child}
              onTool={onTool}
              toolFile={toolFile}
              fileOf={fileOf}
              onOpenFile={onOpenFile}
              onOpenSent={onOpenSent}
              onOpenImage={onOpenImage}
              revertDisabled={revertDisabled}
            />
          )}
        />
      );
    case "subagent":
      return (
        <SubagentRow
          agent={row.agent}
          renderChild={(child) => (
            <Row
              row={child}
              onTool={onTool}
              toolFile={toolFile}
              fileOf={fileOf}
              onOpenFile={onOpenFile}
              onOpenSent={onOpenSent}
              onOpenImage={onOpenImage}
              revertDisabled={revertDisabled}
            />
          )}
        />
      );
    case "image":
      return <AgentImageChip image={row.image} onPress={onOpenImage && (() => onOpenImage(row.image.path))} />;
    case "error":
      return <ErrorRow message={row.message} />;
    case "unreachable":
      return unreachable ? (
        <UnreachableCard
          agentName={unreachable.agentName}
          message={row.message}
          online={unreachable.online}
          hostName={unreachable.hostName}
          onRetry={unreachable.onRetry}
          onRetryInNewChat={unreachable.onRetryInNewChat}
          onStarted={unreachable.onStarted}
        />
      ) : (
        <ErrorRow message={row.message} />
      );
    case "footer":
      return <FooterRow text={row.text} stopped={row.stopped} files={row.files} fileOf={fileOf} onOpenFile={onOpenFile} />;
  }
}

const styles = StyleSheet.create({
  page: { flex: 1 },
  flex: { flex: 1 },
  header: {
    minHeight: 56,
    flexDirection: "row",
    alignItems: "center",
    paddingRight: space.xs,
    borderBottomWidth: StyleSheet.hairlineWidth,
  },
  back: { width: 44, height: 44, alignItems: "center", justifyContent: "center" },
  more: { width: 44, height: 44, alignItems: "center", justifyContent: "center" },
  headerText: { flex: 1, gap: space.xxs },
  dim: { opacity: 0.6 },
  list: { paddingHorizontal: space.lg },
  headerStack: { gap: space.sm, paddingBottom: space.md },
  notice: { paddingHorizontal: space.lg, paddingTop: space.sm },
});
