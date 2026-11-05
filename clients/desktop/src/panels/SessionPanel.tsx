import { memo, useCallback, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { ArrowDown, ArrowUp, GitBranch, GitBranchPlus, Loader2, Paperclip, Square } from "lucide-react";
import {
  effortFor,
  FORK_BUSY_NOTE,
  FORK_OFFLINE_NOTE,
  forkRefusalText,
  forkTurnOf,
  hasCapability,
  historyLaunches,
  historyReply,
  isClientError,
  offersChatRewind,
  offersFork,
  optionUnavailableDetails,
  revertNotes,
  turnGroups,
  type AgentDefaults,
  type AgentSummary,
  type AgentType,
  type ChoiceOption,
  type ForkTarget,
  type HistoryTurn,
  type LiveTurn,
  type OpParams,
  type OptionsRequest,
  type PendingUserMessage,
  type PromptBlock,
  type QueuedFollowUp,
  type RevertChoice,
  type SessionBackground,
  type SessionCommand,
  type SessionOptions,
  type SessionView,
  type TurnGroup,
} from "@zwiim/sdk";
import { PendingAttachments } from "@/attachments/AttachmentChip";
import { RevertDialog } from "@/components/RevertDialog";
import { pastedFiles } from "@/attachments/long-paste";
import { UPLOAD_FAILED_MESSAGE, usePendingAttachments } from "@/attachments/usePendingAttachments";
import { Button } from "@/components/ui/button";
import { errorMessage, useController, useHost, useOnline } from "@/lib/host-context";
import { formatShortcut } from "@/lib/platform";
import { agentChoices, autoConvertLongText, followUpMode, rememberAgentChoices, sendKey } from "@/lib/prefs";
import { isOtherSendKey, isSendKey, sendShortcut } from "@/lib/send-key";
import { usePref } from "@/lib/use-pref";
import { cn } from "@/lib/utils";
import { ContextRing } from "./ContextRing";
import { ControlSelect, OptionsBar, UnattendedBanner } from "./OptionsBar";
import { RequestCard } from "./RequestCard";
import { UnreachableCard } from "./UnreachableCard";
import { ContinuedCard, ForkedFromRow } from "./ContinuedCard";
import { dropForkedDraft, EMPTY_DRAFT, forkedDraft, keepForkedDraft, type ComposerDraft } from "./forkedDraft";
import { PlanStrip } from "./PlanStrip";
import { FollowUpQueue } from "./FollowUpQueue";
import { useSlashMenu } from "./SlashCommands";
import { AgentTerminalCard, AgentTerminalsContext } from "./AgentTerminal";
import { SignInNeededCard } from "./SignInNeededCard";
import {
  BackgroundWaiting,
  carriedBlocks,
  continuedKeys,
  HistoryTurnView,
  KeptStateContext,
  LiveTurnView,
  moveKept,
  sentKeys,
  stopReasonText,
  TurnFooter,
  type TurnFork,
  type KeptState,
} from "./Transcript";

const UNAVAILABLE: Record<string, string> = {
  not_installed: "not installed",
  unsupported_platform: "not supported here",
  disabled: "disabled",
  installing: "setting up",
  needs_network: "setup needs network",
  setup_failed: "setup failed",
};

/** "Codex — not installed"; "Gemini CLI — experimental" for agents Zwiim does not support yet. */
export function agentLabel(a: AgentSummary): string {
  if (!a.available) return `${a.name} — ${UNAVAILABLE[a.reason ?? ""] ?? "unavailable"}`;
  return a.support === "experimental" ? `${a.name} — experimental` : a.name;
}

/** The agents a new session offers: the supported ones (Claude Code, Codex). */
export function offeredAgents(agents: readonly AgentSummary[]): AgentSummary[] {
  return agents.filter((a) => a.support === "supported");
}

/**
 * What a new session of an agent starts with: the agent's advertised options,
 * explicit picks, then the Host's defaults (shared by every device), then the
 * model/effort this client last chose for it — each only while still
 * offered. Effort is the chosen model's own (`effortOn`; none on a model
 * without one). Mode starts at the Host's default mode, else at Auto
 * (unattended) where the agent offers it, else at the agent's own mode,
 * unless picked.
 */
export function newSessionOptions(
  advertised: SessionOptions,
  remembered: { model: string | null; effort: string | null },
  picks: OptionsRequest,
  effortOn: (model: string | null) => ChoiceOption | null = () => advertised.effort,
  hostDefaults?: AgentDefaults,
): { shown: SessionOptions; request: OptionsRequest } {
  const request: OptionsRequest = {};
  const choose = (key: "model" | "effort", option: ChoiceOption | null) => {
    if (!option) return null;
    // A new session starts from the agent's defaults, so the shown value is
    // always sent — even when it matches what the agent last reported.
    const offered = (v: string | null | undefined): v is string => !!v && option.values.some((o) => o.value === v);
    const current = [picks[key], hostDefaults?.[key], remembered[key]].find(offered) ?? option.current;
    if (current) request[key] = current;
    return { ...option, current };
  };
  const mode = advertised.mode;
  const wantMode =
    picks.mode ?? hostDefaults?.mode ?? (mode?.available.includes("unattended") ? "unattended" : undefined);
  if (mode && wantMode && wantMode !== mode.current && mode.available.includes(wantMode)) {
    request.mode = wantMode;
  }
  const model = choose("model", advertised.model);
  return {
    shown: {
      model,
      effort: choose("effort", effortOn(model?.current ?? null)),
      mode: mode && request.mode ? { ...mode, current: request.mode, label: "" } : mode,
    },
    request,
  };
}

/**
 * The agent to preselect: the one picked here if still usable, else the
 * Host's default agent, else the first usable supported one, else the first
 * usable one.
 */
export function defaultAgent(agents: AgentSummary[], preferred: string | null): string | null {
  const usable = agents.filter((a) => a.available);
  return (
    usable.find((a) => a.agent === preferred) ??
    usable.find((a) => a.default) ??
    usable.find((a) => a.support === "supported") ??
    usable[0]
  )?.agent ?? null;
}

/** How often, and how many times, to look for an agent's options being learned. */
const LEARN_POLL_MS = 2000;
const LEARN_POLLS = 30;

/**
 * The agent a new session starts with and its options: the agent picked here
 * (else the first usable one), and the model/effort/mode picks for it.
 */
export function useNewSessionAgent() {
  const controller = useController();
  const listed = useHost((s) => s.agents);
  const agents = useMemo(() => listed && offeredAgents(listed), [listed]);
  // An agent's defaults move with its last session (a model without effort,
  // say), so a new session starts from what the host reports now.
  useEffect(() => {
    void controller.refreshAgents().catch(() => {});
  }, [controller]);
  const [chosen, setChosen] = useState<string | null>(null);
  const agent = agents ? defaultAgent(agents, chosen) : null;
  const summary = agents?.find((a) => a.agent === agent) ?? null;
  const advertised = summary?.options ?? null;
  // A supported agent that has not run on this host yet: the host learns its
  // options in the background, so read the agents again until they arrive.
  const learning = !!summary?.available && summary.support === "supported" && !advertised;
  useEffect(() => {
    if (!learning) return;
    let polls = 0;
    const timer = window.setInterval(() => {
      if (++polls > LEARN_POLLS) window.clearInterval(timer);
      else void controller.refreshAgents().catch(() => {});
    }, LEARN_POLL_MS);
    return () => window.clearInterval(timer);
  }, [controller, learning, agent]);
  const [picks, setPicks] = useState<OptionsRequest>({});
  const start =
    advertised && summary
      ? newSessionOptions(
          advertised,
          agentChoices(summary.agent),
          picks,
          (model) => effortFor(summary, model),
          summary.defaults,
        )
      : null;
  return {
    agents,
    agent,
    /** What the agent offers before any pick. */
    advertised,
    /** Its slash commands, from its last session on this host. */
    commands: agents?.find((a) => a.agent === agent)?.commands ?? null,
    start,
    pick(req: OptionsRequest) {
      setPicks((p) => ({ ...p, ...req }));
      if (agent) rememberAgentChoices(agent, { model: req.model, effort: req.effort });
    },
    choose(next: string) {
      setChosen(next);
      setPicks({});
    },
  };
}

/** The agent picker and its options, as composer controls. */
export function NewSessionControls({
  picker,
  disabled,
}: {
  picker: ReturnType<typeof useNewSessionAgent>;
  disabled: boolean;
}) {
  const { agents, agent, start } = picker;
  if (!agents || agent === null) return null;
  return (
    <>
      <ControlSelect
        label="Agent"
        value={agent}
        onChange={picker.choose}
        options={agents.map((a) => ({ value: a.agent, label: agentLabel(a), disabled: !a.available }))}
      />
      {start && <OptionsBar options={start.shown} disabled={disabled} onChange={picker.pick} />}
    </>
  );
}

/** `session.start` for a new session's first message. */
export function startParams(
  picker: ReturnType<typeof useNewSessionAgent>,
  workspace_id: string,
  blocks: PromptBlock[],
  client_message_id: string,
): OpParams<"session.start"> | null {
  if (picker.agent === null) return null;
  const options = picker.start?.request ?? {};
  return {
    workspace_id,
    agent: picker.agent,
    blocks,
    client_message_id,
    ...(Object.keys(options).length > 0 ? { options } : {}),
  };
}

/** Running a slash command in the agent's CLI, over the composer. */
interface AgentTerminal {
  agentName: string;
  run(command: string): Promise<void>;
}

/**
 * Where the composer runs a slash command the agent does not run itself
 * (`/mcp`, `/login`): the agent's own CLI, in a terminal of the workspace
 * shown over the composer. Only for an agent the host installed, so its CLI
 * is there. A new command replaces the workspace's last one.
 */
function useAgentTerminal(workspaceId: string, agent: AgentType | null | undefined): AgentTerminal | null {
  const controller = useController();
  const terminals = useContext(AgentTerminalsContext);
  const summary = useHost((s) => s.agents?.find((a) => a.agent === agent) ?? null);
  const offered = useHost((s) => hasCapability(s.connection.compat, "terminal.agent"));
  if (!terminals || !offered || !summary?.adapter?.installed || summary.adapter.source !== "managed") return null;
  return {
    agentName: summary.name,
    async run(command) {
      const last = terminals.runs[workspaceId];
      const terminalId = await controller.spawnTerminal(workspaceId, { agent: summary.agent, command });
      terminals.open(workspaceId, { terminalId, command });
      if (last) {
        // Not ended, it shows in the dock, where the user can end it.
        controller.client
          .request("terminal.kill", { terminal_id: last.terminalId })
          .catch(() => terminals.release(last.terminalId));
      }
    },
  };
}

export function SessionPanel(props: {
  workspaceId: string;
  sessionId: string | null;
  onSessionStarted(id: string): void;
  /** A session that may be in another workspace: a fork, or the one it came from. */
  onOpenSession?(workspaceId: string, id: string): void;
  /** A repository-folder session's Continue: a new worktree for its root. */
  onContinue?(): void;
}) {
  const removed = useHost((s) => s.workspaces?.find((w) => w.workspace_id === props.workspaceId)?.removed ?? false);
  // On a current Host a root's own sessions are read-only history (§5.3, D8).
  const root = useHost(
    (s) =>
      hasCapability(s.connection.compat, "workspace.publish") &&
      s.workspaces?.find((w) => w.workspace_id === props.workspaceId)?.parent_workspace_id === null,
  );
  if (props.sessionId === null) {
    if (root) {
      return (
        <div className="flex h-full flex-col items-center justify-center gap-3 p-6 text-center">
          <p className="text-muted-foreground">Work in this repository continues in a worktree.</p>
          <ContinueButton onContinue={props.onContinue} />
        </div>
      );
    }
    if (removed) {
      return (
        <p className="text-muted-foreground p-6 text-center">
          This worktree was archived. Its sessions stay readable, but no new work can start here.
        </p>
      );
    }
    return <NewSession key={props.workspaceId} workspaceId={props.workspaceId} onStarted={props.onSessionStarted} />;
  }
  return (
    <OpenSession
      key={props.sessionId}
      workspaceId={props.workspaceId}
      sessionId={props.sessionId}
      removed={removed}
      root={root}
      onContinue={props.onContinue}
      onSessionStarted={props.onSessionStarted}
      onOpenSession={props.onOpenSession}
    />
  );
}

function ContinueButton({ onContinue }: { onContinue?(): void }) {
  const online = useOnline();
  return (
    <Button
      size="xs"
      variant="brand"
      className="shrink-0"
      disabled={!online || !onContinue}
      title={online ? "A new worktree of this repository, with an empty composer" : "The Host is offline"}
      onClick={onContinue}
    >
      <GitBranchPlus />
      Continue in a new worktree
    </Button>
  );
}

function NewSession({ workspaceId, onStarted }: { workspaceId: string; onStarted(id: string): void }) {
  const controller = useController();
  const online = useOnline();
  const workspace = useHost((s) => s.workspaces?.find((w) => w.workspace_id === workspaceId) ?? null);
  const picker = useNewSessionAgent();
  const { agents, agent, start } = picker;
  const terminal = useAgentTerminal(workspaceId, agent);
  return (
    <div className="flex h-full flex-col">
      <div className="flex flex-1 flex-col items-center justify-center gap-2 p-6 text-center">
        <p className="text-xl font-semibold tracking-tight">Start a session</p>
        {workspace && (
          <p className="text-muted-foreground flex items-center gap-1.5 text-sm">
            {workspace.alias ?? workspace.name}
            {workspace.branch && (
              <>
                <GitBranch className="size-3.5" />
                <span className="font-mono text-xs">{workspace.branch}</span>
              </>
            )}
          </p>
        )}
        {agents === null && <p className="text-muted-foreground text-xs">Checking which agents this host can run…</p>}
        {agents !== null && agent === null && <NoAgents />}
      </div>
      <Composer
        workspaceId={workspaceId}
        placeholder="Describe the task. Ask for a task list to track progress"
        disabled={!online || agent === null}
        unattended={start?.shown.mode?.current === "unattended"}
        banner={start && <UnattendedBanner options={start.shown} disabled={!online} onChange={picker.pick} />}
        controls={<NewSessionControls picker={picker} disabled={!online} />}
        commands={picker.commands}
        terminal={terminal}
        notice={<AgentTerminalCard workspaceId={workspaceId} />}
        onSend={async (blocks, client_message_id) => {
          const params = startParams(picker, workspaceId, blocks, client_message_id);
          if (params) onStarted(await controller.startSession(params));
        }}
      />
    </div>
  );
}

export function NoAgents() {
  const controller = useController();
  return (
    <div className="text-muted-foreground max-w-sm text-center text-xs" data-testid="no-agents">
      <p>No agent is available on this host.</p>
      <p className="mt-1">
        Install one on the host (for example Claude Code), then{" "}
        <button className="hover:text-foreground underline underline-offset-2 transition-colors" onClick={() => void controller.refreshAgents().catch(() => {})}>
          check again
        </button>
        .
      </p>
    </div>
  );
}

function OpenSession({
  workspaceId,
  sessionId,
  removed: gone,
  root,
  onContinue,
  onSessionStarted,
  onOpenSession,
}: {
  workspaceId: string;
  sessionId: string;
  removed: boolean;
  /** It ran in the repository folder: read-only, with Continue in place of the composer. */
  root: boolean;
  onContinue?(): void;
  /** A plan was handed off to a new session in this workspace. */
  onSessionStarted(id: string): void;
  onOpenSession?(workspaceId: string, id: string): void;
}) {
  // Neither a removed worktree nor the repository folder takes new work.
  const removed = gone || root;
  const controller = useController();
  const online = useOnline();
  const view = useHost((s) => s.sessions[sessionId]);
  const summary = useHost((s) => s.sessionsByWorkspace[workspaceId]?.find((x) => x.session_id === sessionId));
  const agentName = useHost((s) => s.agents?.find((a) => a.agent === summary?.agent)?.name ?? null);
  const handOff = useHost((s) => hasCapability(s.connection.compat, "session.plan_hand_off"));
  const canRetry = useHost((s) => hasCapability(s.connection.compat, "session.retry"));
  const planned = useHost((s) => hasCapability(s.connection.compat, "agent.progress"));
  const canFollowUp = useHost((s) => hasCapability(s.connection.compat, "session.follow_ups"));
  const canSteer = useHost((s) => hasCapability(s.connection.compat, "session.steer"));
  const canFork = useHost((s) => offersFork(s.connection.compat));
  const followUps = summary?.follow_ups ?? NO_FOLLOW_UPS;
  const compat = useHost((s) => s.connection.compat);
  // Before its agent announces them (or while it is away), the commands it
  // offered last.
  const agentCommands = useHost((s) => s.agents?.find((a) => a.agent === summary?.agent)?.commands ?? null);
  const terminal = useAgentTerminal(workspaceId, removed ? null : summary?.agent);
  const [cancelError, setCancelError] = useState<string | null>(null);
  const [confirmCancel, setConfirmCancel] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  // The message a reset is being asked about, and what the last one left to say.
  const [resetting, setResetting] = useState<{ turnId: string; chat: boolean } | null>(null);
  const [resetNotes, setResetNotes] = useState<string[]>([]);
  // A fork on its way, and which turn's the Host last refused.
  const [forking, setForking] = useState(false);
  const [forkError, setForkError] = useState<{ turnId: string; message: string } | null>(null);
  // A steer the agent could not take mid-turn, which waits instead.
  const [steerQueued, setSteerQueued] = useState(false);
  // What the plan's Continue was refused with.
  const [planNotice, setPlanNotice] = useState<string | null>(null);
  // What is open in the history, kept while the session is open.
  const [kept] = useState<KeptState>(() => new Map());
  const scroller = useRef<HTMLDivElement>(null);
  const history = view?.history;
  const turns = history ?? NO_TURNS;
  // What a new turn opens is kept under its message until the history has the
  // turn, then under its reply there, before that reply's row first draws. A
  // history read before the turn ended (back after it ended while away) waits
  // for the fresh one.
  const sent = useRef(new Map<string, PendingUserMessage>());
  for (const t of [view?.settled, view?.live])
    if (t?.userMessage) sent.current.set(t.userMessage.message_id, t.userMessage);
  if (history && !view?.historyStale) {
    for (const [id, message] of sent.current) {
      if (id === view?.settled?.userMessage?.message_id || id === view?.live.userMessage?.message_id) continue;
      const reply = historyReply(history, message);
      if (reply) moveKept(kept, `live:${id}/`, `${reply.turn_id}/`);
      sent.current.delete(id);
    }
  }
  // Only the history rows in and near view are mounted; the live turn and
  // what follows it always are.
  const list = useRef<HTMLDivElement>(null);
  const [listTop, setListTop] = useState(0);
  const rows = useVirtualizer({
    count: turns.length,
    getScrollElement: () => scroller.current,
    estimateSize: estimateRow,
    getItemKey: useCallback((i: number) => turns[i]!.turn_id, [turns]),
    gap: ROW_GAP_PX,
    overscan: ROW_OVERSCAN,
    scrollMargin: listTop,
    // Attaching to the transcript leaves it where it is.
    initialOffset: () => scroller.current?.scrollTop ?? 0,
  });
  useLayoutEffect(() => {
    const top = list.current?.offsetTop ?? 0;
    if (top !== listTop) setListTop(top);
  });
  const { atBottom, jump, scrollProps } = useStickToBottom(
    scroller,
    [view?.history, view?.live, view?.settled, view?.pendingRequest, view?.lastError, view?.background],
    rows.getTotalSize(),
  );

  useEffect(() => {
    controller.openSession(sessionId);
    return () => controller.closeSession(sessionId);
  }, [controller, sessionId]);

  const running = view?.agentStatus === "prompting";
  // A starting agent may be about to run a queued first turn.
  const starting = view?.agentStatus === "connecting";
  useEffect(() => {
    if (!running) {
      setConfirmCancel(false);
      setSteerQueued(false);
    }
  }, [running]);
  const stopped = running ? null : stopReasonText(view?.lastStopReason ?? null);
  const [optionsBusy, setOptionsBusy] = useState(false);
  const [optionsError, setOptionsError] = useState<string | null>(null);
  const footers = useMemo(() => new Map((history ? turnGroups(history) : []).map((g) => [g.end, g])), [history]);
  const lastFooter = useMemo(() => Math.max(-1, ...footers.keys()), [footers]);
  const composerDraft = useRef(EMPTY_DRAFT);
  const [startingDraft] = useState(() => forkedDraft(sessionId));
  useEffect(() => dropForkedDraft(sessionId), [sessionId]);
  const fork = useCallback(
    async (turnId: string, target: ForkTarget) => {
      setForking(true);
      setForkError(null);
      try {
        const forked = await controller.forkSession(sessionId, turnId, target);
        keepForkedDraft(forked.session_id, composerDraft.current);
        onOpenSession?.(forked.workspace_id, forked.session_id);
      } catch (e) {
        setForkError({ turnId, message: forkRefusalText(e) ?? errorMessage(e) });
      } finally {
        setForking(false);
      }
    },
    [controller, sessionId, onOpenSession],
  );
  const forkDisabled = !online ? FORK_OFFLINE_NOTE : running || forking ? FORK_BUSY_NOTE : null;
  // One object per turn while nothing about forking changes, so streaming leaves the rows memoized.
  const forks = useMemo(() => {
    const out = new Map<number, TurnFork>();
    if (!canFork || removed || !history) return out;
    for (const [end, group] of footers) {
      const forkTurn = forkTurnOf(history, group);
      if (!forkTurn) continue;
      out.set(end, {
        turnId: forkTurn.turn_id,
        targets: forkTurn.fork_targets!,
        older: end !== lastFooter,
        disabledReason: forkDisabled,
        error: forkError?.turnId === forkTurn.turn_id ? forkError.message : null,
        onFork: fork,
      });
    }
    return out;
  }, [canFork, removed, history, footers, lastFooter, forkDisabled, forkError, fork]);
  // A prompt already in the history while the agent still answers it has no
  // row yet, nor once it has finished until the history has the turn: that
  // live turn carries on the history's last.
  const tail = running ? view?.live : view?.settled;
  // So while a finished turn the history lacks carries on it, the history's
  // last turn reads unended too, with no footer.
  const runningEnd = tail && !tail.userMessage && history?.length ? history.length - 1 : -1;
  // A run of tool calls it ends with folds with the live reply's first ones
  // there, and what that reply opens is kept as the history will key it;
  // not when a finished turn the history lacks comes between them.
  const continues = tail && runningEnd >= 0 && !(running && view?.settled) ? tail : undefined;
  const carried = continues ? carriedBlocks(turns[runningEnd]!, continues) : 0;
  const continued = continues ? turns[runningEnd] : undefined;
  const keys = useMemo(() => continued && continuedKeys(continued), [continued]);
  // Each launch in the history, by its turn: a background sub-agent can keep
  // working after its turn, and its live work shows under its card there.
  const launchTurns = useMemo(
    () => new Map(turns.flatMap((t, i) => [...historyLaunches([t])].map((id) => [id, i] as const))),
    [turns],
  );
  const earlier = useMemo(() => new Set(launchTurns.keys()), [launchTurns]);
  const lateTurns = new Set(Object.keys(view?.live.subagents ?? {}).flatMap((id) => launchTurns.get(id) ?? []));
  // A session started to finish another's plan: its first message, the
  // prompt the Host wrote, shows as a card.
  const planFrom = planned ? (view?.continues ?? null) : null;
  const firstUser = planFrom ? turns.findIndex((t) => t.role === "user") : -1;
  // Not in the history yet: the live turn's message is the first.
  const liveFirst = planFrom && firstUser < 0 ? (view?.settled ?? view?.live)?.userMessage : undefined;
  const firstBlocks = firstUser >= 0 ? turns[firstUser]!.blocks : liveFirst?.blocks;
  const firstCard = useMemo(
    () =>
      planFrom && (
        <ContinuedCard continues={planFrom} attachments={firstBlocks ?? []} onOpenSession={onSessionStarted} />
      ),
    [planFrom, firstBlocks, onSessionStarted],
  );
  // The can't-reach card shows in place of the error line, and of the plan's resume bar.
  const unreachable = view?.lastError?.code === "agent_unreachable" && canRetry;
  const strip = view && (
    <PlanStrip
      view={view}
      root={root}
      archived={gone}
      cardShown={unreachable}
      onOpenSession={onSessionStarted}
      onNotice={setPlanNotice}
    />
  );
  const empty =
    view?.history?.length === 0 && !view.settled && !view.live.userMessage && !view.live.message && !running;

  async function changeOptions(request: OptionsRequest) {
    setOptionsBusy(true);
    setOptionsError(null);
    try {
      await controller.setSessionOptions(sessionId, request);
      if (summary?.agent) rememberAgentChoices(summary.agent, { model: request.model, effort: request.effort });
    } catch (e) {
      // The host changed nothing; the controls keep showing what the agent holds.
      const unavailable = optionUnavailableDetails(e);
      setOptionsError(
        unavailable
          ? `The agent does not offer ${unavailable.value} for ${unavailable.option} now; nothing was changed.`
          : errorMessage(e),
      );
    } finally {
      setOptionsBusy(false);
    }
  }

  async function reset(turnId: string, choice: RevertChoice) {
    setResetNotes([]);
    // A chat rewind arrives as a fresh snapshot; nothing to refetch.
    const result = await controller.revertSession(sessionId, turnId, choice);
    setResetNotes(revertNotes(result, choice));
  }

  async function cancel() {
    setCancelError(null);
    setCancelling(true);
    try {
      await controller.client.request("session.cancel", { session_id: sessionId });
      setConfirmCancel(false);
    } catch (e) {
      setCancelError(errorMessage(e));
    } finally {
      setCancelling(false);
    }
  }

  // One reset handler for every row, so a row redraws only when it changes.
  const latest = useRef({ turns, options: view?.options, compat });
  useLayoutEffect(() => {
    latest.current = { turns, options: view?.options, compat };
  });
  const askReset = useCallback((i: number) => {
    const { turns, options, compat } = latest.current;
    const t = turns[i];
    if (t) setResetting({ turnId: t.turn_id, chat: offersChatRewind(turns[i - 1], options, compat) });
  }, []);

  const shown = rows.getVirtualItems();
  const total = rows.getTotalSize();
  const first = shown[0];
  const last = shown[shown.length - 1];

  const options = view?.options && !removed ? view.options : null;
  const optionsDisabled = running || starting || !online || optionsBusy;
  return (
    <div className="flex h-full min-h-0 flex-col">
      <header className="flex h-11 shrink-0 items-center gap-2.5 px-5">
        <span className="truncate text-sm font-medium">{summary?.title || "Session"}</span>
        {(agentName ?? summary?.agent) && (
          <span className="text-muted-foreground shrink-0 text-xs">{agentName ?? summary?.agent}</span>
        )}
        <SessionStatus view={view} />
      </header>
      <div className="relative flex min-h-0 flex-1 flex-col">
        <div
          ref={scroller}
          {...scrollProps}
          // The rows keep the view in place as they are measured; the browser does not as well.
          className="relative min-h-0 flex-1 overflow-y-auto [overflow-anchor:none]"
          data-testid="transcript"
        >
          <div className="mx-auto flex w-full max-w-3xl flex-col gap-5 px-6 pt-4 pb-6">
            {view?.history === null && (
              <p className="text-muted-foreground flex items-center gap-1.5 text-xs">
                <Loader2 className="size-3 animate-spin" /> Loading the conversation…
              </p>
            )}
            {view?.forkedFrom && <ForkedFromRow from={view.forkedFrom} onOpenSession={onOpenSession} />}
            {empty && <p className="text-muted-foreground py-10 text-center">No messages yet.</p>}
            {/* What is open in the history and the live turn that carries on it. */}
            <KeptStateContext.Provider value={kept}>
              {turns.length > 0 && (
                <div
                  ref={list}
                  className="flex flex-col gap-5"
                  // The rows above and below the mounted ones, as space.
                  style={{
                    paddingTop: first ? first.start - listTop : 0,
                    paddingBottom: last ? total - (last.end - listTop) : total,
                  }}
                  data-testid="history"
                >
                  {shown.map(({ index: i, key }) => (
                    <HistoryRow
                      key={key}
                      index={i}
                      measure={rows.measureElement}
                      turn={turns[i]!}
                      group={i !== runningEnd ? footers.get(i) : undefined}
                      fork={forks.get(i)}
                      onRevert={removed ? undefined : askReset}
                      revertDisabled={running || !online}
                      background={view?.background ?? null}
                      ended={i !== runningEnd}
                      live={lateTurns.has(i) ? view?.live : undefined}
                      carry={i === runningEnd && carried > 0 ? continues : undefined}
                      carried={i === runningEnd ? carried : 0}
                      card={i === firstUser ? firstCard : undefined}
                    />
                  ))}
                </div>
              )}
              {view?.settled && (
                <LiveTurnView
                  turn={view.settled}
                  userCard={liveFirst && liveFirst === view.settled.userMessage ? firstCard : undefined}
                  background={view.background}
                  earlier={earlier}
                  from={continues === view.settled ? carried : 0}
                  keys={continues === view.settled ? keys : sentKeys(view.settled)}
                />
              )}
              {view && (
                <LiveTurnView
                  turn={view.live}
                  userCard={liveFirst && liveFirst === view.live.userMessage ? firstCard : undefined}
                  streaming={running}
                  reconnecting={view.reconnecting}
                  animate
                  background={view.background}
                  earlier={earlier}
                  from={continues === view.live ? carried : 0}
                  keys={continues === view.live ? keys : sentKeys(view.live)}
                />
              )}
            </KeptStateContext.Provider>
            {/* The turn is over but its background work is not: say so where the turn ended. */}
            <BackgroundWaiting background={view?.background} className="animate-fade-in motion-reduce:animate-none" />
            {stopped && (
              <p className="text-muted-foreground text-xs" data-testid="stop-reason">
                {stopped}
              </p>
            )}
            {view?.lastError &&
              (unreachable ? (
                <UnreachableCard
                  key={view.errorSerial}
                  agentName={agentName ?? summary?.agent ?? "Agent"}
                  message={view.lastError.message}
                  sessionId={sessionId}
                  online={online}
                  onStarted={onSessionStarted}
                />
              ) : view.lastError.code === "turn_failed_auth_required" ? (
                <SignInNeededCard key={view.errorSerial} message={view.lastError.message} />
              ) : (
                <p className="text-destructive text-xs" role="alert">
                  {view.lastError.message}
                </p>
              ))}
            {view?.pendingRequest && (
              <RequestCard
                key={view.pendingRequest.request_id}
                request={view.pendingRequest}
                disabled={!online}
                handOff={handOff}
                onAnswer={async (answer) => {
                  const result = await controller.client.request("session.respond", {
                    session_id: sessionId,
                    request_id: view.pendingRequest!.request_id,
                    ...answer,
                  });
                  if (result.session_id) onSessionStarted(result.session_id);
                }}
              />
            )}
          </div>
        </div>
        {!atBottom && (
          <Button
            className="bg-popover shadow-pop animate-fade-in absolute bottom-3 left-1/2 -translate-x-1/2 rounded-full"
            size="xs"
            variant="outline"
            onClick={jump}
          >
            <ArrowDown /> Jump to latest
          </Button>
        )}
      </div>
      {root ? (
        <div className="shrink-0 px-6 pt-1 pb-4">
          <div className="mx-auto flex w-full max-w-3xl flex-col gap-2">
            {planNotice && (
              <p className="text-destructive text-xs" role="alert">
                {planNotice}
              </p>
            )}
            {strip}
            <div
              // In the composer's place, on the composer's surface.
              className="bg-card ring-border relative flex items-center gap-3 rounded-2xl px-4 py-3 text-ui ring-1"
              data-testid="root-continue"
            >
              <span className="text-muted-foreground min-w-0 flex-1">This session ran in the repository folder.</span>
              <ContinueButton onContinue={onContinue} />
            </div>
          </div>
        </div>
      ) : (
        <Composer
          workspaceId={workspaceId}
          initialDraft={startingDraft}
          draftRef={composerDraft}
          placeholder={
            removed
              ? "This worktree was archived — the session is read-only"
              : running
                ? canFollowUp
                  ? "Add a follow-up"
                  : "Draft your next message while the agent works"
                : "Send a message"
          }
          // A running turn only holds the send back: the next message can be
          // drafted, or with a host that keeps follow-ups, queued.
          disabled={removed || !online}
          busy={running && !canFollowUp}
          queueing={canFollowUp && (running || followUps.length > 0)}
          steerable={canSteer && running}
          unattended={options?.mode?.current === "unattended"}
          commands={view?.commands ?? agentCommands}
          terminal={running ? null : terminal}
          notice={
            <>
              <AgentTerminalCard workspaceId={workspaceId} />
              {canFollowUp && followUps.length > 0 && !removed && (
                <FollowUpQueue sessionId={sessionId} items={followUps} running={running} disabled={!online} />
              )}
              {steerQueued && (
                <p className="text-muted-foreground text-xs" role="status" data-testid="steer-queued">
                  The agent could not take it mid-turn, so it waits in the queue.
                </p>
              )}
              {resetNotes.map((note) => (
                <p key={note} className="text-muted-foreground text-xs" role="status" data-testid="reset-note">
                  {note}
                </p>
              ))}
              {optionsError && (
                <p className="text-destructive text-xs" role="alert" data-testid="options-error">
                  {optionsError}
                </p>
              )}
              {cancelError && (
                <p className="text-destructive text-xs" role="alert">
                  {cancelError}
                </p>
              )}
              {planNotice && (
                <p className="text-destructive text-xs" role="alert">
                  {planNotice}
                </p>
              )}
              {running && !removed && confirmCancel && (
                <div
                  className="bg-card animate-enter flex items-center gap-2 rounded-lg px-3 py-2 text-xs shadow-sm"
                  data-testid="confirm-cancel"
                >
                  <span className="flex-1">Stop the agent's current turn?</span>
                  <Button size="xs" variant="ghost" disabled={cancelling} onClick={() => setConfirmCancel(false)}>
                    Keep going
                  </Button>
                  <Button size="xs" variant="destructive" disabled={!online || cancelling} onClick={cancel}>
                    {cancelling && <Loader2 className="animate-spin" />}
                    Stop
                  </Button>
                </div>
              )}
            </>
          }
          dock={strip}
          banner={
            options && (
              <UnattendedBanner
                options={options}
                disabled={optionsDisabled}
                onChange={(req) => void changeOptions(req)}
              />
            )
          }
          controls={
            options && (
              <>
                {(agentName ?? summary?.agent) && (
                  <span className="text-muted-foreground flex min-w-0 items-center gap-1 pl-2 text-xs">
                    <span className="truncate">{agentName ?? summary?.agent}</span>
                    <span aria-hidden>·</span>
                  </span>
                )}
                <OptionsBar
                  options={options}
                  disabled={optionsDisabled}
                  disabledReason={running ? "Options change between turns" : starting ? "The agent is starting…" : null}
                  onChange={(req) => void changeOptions(req)}
                />
                <ContextRing view={view} agent={agentName ?? summary?.agent ?? null} />
              </>
            )
          }
          action={
            running &&
            !removed && (
              <button
                type="button"
                className="bg-foreground text-background enabled:hover:bg-foreground/85 ease-std grid size-8 shrink-0 place-items-center rounded-full transition-[background-color,opacity,transform] duration-120 enabled:active:scale-[.96] disabled:opacity-50"
                aria-label="Stop agent"
                title="Stop the agent's current turn"
                data-testid="stop-agent"
                disabled={!online || confirmCancel}
                onClick={() => setConfirmCancel(true)}
              >
                <Square className="size-3 fill-current" />
              </button>
            )
          }
          onSend={async (blocks, client_message_id, steer) => {
            setResetNotes([]);
            setSteerQueued(false);
            const params = { session_id: sessionId, blocks, client_message_id };
            if (!canFollowUp) return controller.sendMessage(params);
            const outcome = await controller.sendFollowUp(steer ? { ...params, steer } : params);
            setSteerQueued(steer && outcome === "queued");
          }}
        />
      )}
      {resetting && (
        <RevertDialog
          chat={resetting.chat}
          disabled={!online || running}
          onRevert={(choice) => reset(resetting.turnId, choice)}
          onClose={() => setResetting(null)}
        />
      )}
    </div>
  );
}

const NO_TURNS: readonly HistoryTurn[] = [];
const NO_FOLLOW_UPS: readonly QueuedFollowUp[] = [];

/** A history row's height until it is measured, the space between rows (gap-5), and the rows mounted past the view. */
const ROW_ESTIMATE_PX = 160;
const ROW_GAP_PX = 20;
const ROW_OVERSCAN = 4;
const estimateRow = () => ROW_ESTIMATE_PX;

/**
 * One history turn and the row under it (how long it ran, what it changed),
 * measured where it sits. It redraws only when what it shows changes, not as
 * the live turn streams.
 */
const HistoryRow = memo(function HistoryRow({
  index,
  measure,
  turn,
  group,
  fork,
  onRevert,
  revertDisabled,
  background,
  ended,
  live,
  carry,
  carried,
  card,
}: {
  index: number;
  measure(el: HTMLElement | null): void;
  turn: HistoryTurn;
  group: TurnGroup | undefined;
  fork: TurnFork | undefined;
  onRevert?(index: number): void;
  revertDisabled: boolean;
  background: SessionBackground | null;
  ended: boolean;
  /** Only for a turn whose sub-agent is still adding to the live turn, so the rest do not redraw as it streams. */
  live: LiveTurn | undefined;
  /** The live turn that carries on this one's run, running or just finished and not yet in the history. */
  carry: LiveTurn | undefined;
  /** How many of `carry`'s first blocks fold with this turn's last run. */
  carried: number;
  /** Shown in place of the user message: a continued plan's first message is a card. */
  card?: React.ReactNode;
}) {
  const revert = useCallback(() => onRevert?.(index), [onRevert, index]);
  return (
    <div ref={measure} data-index={index} className="flex flex-col gap-5">
      <HistoryTurnView
        turn={turn}
        onRevert={onRevert && revert}
        revertDisabled={revertDisabled}
        background={background}
        ended={ended}
        live={live}
        carry={carry}
        carried={carried}
        userCard={card}
      />
      {group && <TurnFooter group={group} stateKey={`${turn.turn_id}/footer`} fork={fork} />}
    </div>
  );
});

/** Where the agent is: a dot and a word, next to the session title. */
function SessionStatus({ view }: { view: SessionView | undefined }) {
  if (!view) return null;
  let text: string;
  let dot: string;
  if (view.attach === "live") {
    const s = view.agentStatus;
    text = s === "prompting" ? "Working" : s === "connecting" ? "Starting…" : "Agent ready";
    dot =
      s === "prompting"
        ? "bg-muted-foreground animate-breathe"
        : s === "connecting"
          ? "bg-faint animate-breathe"
          : "bg-done";
  } else if (view.attach === "attaching") {
    text = "Attaching…";
    dot = "bg-faint animate-breathe";
  } else if (view.attach === "error") {
    text = view.attachError?.message ?? "Error";
    dot = "bg-destructive";
  } else if (view.attach === "detached") {
    text = "Idle";
    dot = "bg-faint";
  } else {
    return null;
  }
  return (
    <span className="text-muted-foreground flex min-w-0 items-center gap-1.5 text-xs" data-testid="session-status">
      <span className={cn("size-2 shrink-0 rounded-full", dot)} />
      <span className="truncate">{text}</span>
    </span>
  );
}

/**
 * Follow new content while the reader is at the bottom, gliding down to what
 * arrives; once they scroll up, stay put and offer a jump back.
 */
/**
 * How quickly the transcript follows what arrives: each frame it closes this
 * much of the distance left per 16ms, so a burst of updates is one steady
 * ease-out instead of a scroll restarted for each of them.
 */
const FOLLOW_RATE = 0.16;

function useStickToBottom(ref: React.RefObject<HTMLDivElement | null>, deps: unknown[], size: number) {
  const pinned = useRef(true);
  // Our own glide is under way: its scrolls are not the reader leaving.
  const gliding = useRef(false);
  const frame = useRef(0);
  // The transcript itself moved its end (it opened, or content arrived) since
  // the reader last did anything: rows measured now follow that end. A card
  // the reader opens grows a row as well, and must leave the view where it is.
  const settling = useRef(true);
  const [atBottom, setAtBottom] = useState(true);

  const stop = useCallback(() => {
    cancelAnimationFrame(frame.current);
    frame.current = 0;
    gliding.current = false;
  }, []);

  const glide = useCallback(
    (el: HTMLDivElement, smooth: boolean) => {
      const reduce = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
      if (!smooth || reduce || typeof requestAnimationFrame !== "function") {
        stop();
        el.scrollTop = el.scrollHeight;
        return;
      }
      gliding.current = true;
      // One glide at a time; a running one already heads for the new bottom.
      if (frame.current) return;
      let last = performance.now();
      const step = (now: number) => {
        const target = Math.max(0, el.scrollHeight - el.clientHeight);
        const left = target - el.scrollTop;
        if (left <= 1 || !gliding.current) {
          if (left <= 1) el.scrollTop = target;
          frame.current = 0;
          gliding.current = false;
          return;
        }
        const t = 1 - Math.pow(1 - FOLLOW_RATE, (now - last) / 16);
        last = now;
        el.scrollTop += Math.max(1, left * t);
        frame.current = requestAnimationFrame(step);
      };
      frame.current = requestAnimationFrame(step);
    },
    [stop],
  );

  useEffect(() => stop, [stop]);

  const onScroll = useCallback(() => {
    const el = ref.current;
    if (!el) return;
    if (gliding.current) return;
    const bottom = el.scrollHeight - el.scrollTop - el.clientHeight < 48;
    pinned.current = bottom;
    setAtBottom(bottom);
  }, [ref]);

  // The reader takes over: the glide stops where it is, and what they open
  // or close does not move the view.
  const onIntent = useCallback(() => {
    settling.current = false;
    if (gliding.current) stop();
  }, [stop]);

  const jump = useCallback(() => {
    const el = ref.current;
    if (!el) return;
    settling.current = true;
    glide(el, true);
    pinned.current = true;
    setAtBottom(true);
  }, [ref, glide]);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    settling.current = true;
    if (!pinned.current) return;
    // A little more to show glides; a conversation just loaded is shown at once.
    const behind = el.scrollHeight - el.scrollTop - el.clientHeight;
    glide(el, behind > 1 && behind < el.clientHeight);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);

  // History rows measured while following (a session just opened, a turn just
  // added) move the end: back to it at once. A glide under way follows it. A
  // row the reader grew by opening something in it stays where it is.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el || !pinned.current || !settling.current || frame.current) return;
    el.scrollTop = el.scrollHeight;
  }, [ref, size]);

  return {
    atBottom,
    jump,
    scrollProps: { onScroll, onWheel: onIntent, onTouchMove: onIntent, onPointerDown: onIntent, onKeyDown: onIntent },
  };
}

const COMPOSER_MAX_PX = 240;

/**
 * The control surface for the agent's next turn: the message, and in its
 * toolbar the agent's options and send (or stop while it works).
 */
function Composer(props: {
  /** Where attached files are uploaded. */
  workspaceId: string;
  placeholder: string;
  /** What the message and its files start as. */
  initialDraft?: ComposerDraft;
  /** Kept up to date with the message and its files, for the panel to read. */
  draftRef?: React.RefObject<ComposerDraft>;
  disabled?: boolean;
  /** The agent is working: the message can be written but not sent. */
  busy?: boolean;
  /** Send queues the message behind the running turn or held follow-ups; send shows beside `action`. */
  queueing?: boolean;
  /**
   * The running turn can take the message: send steers or queues as the user
   * chose, and Mod+Shift+Enter does the other.
   */
  steerable?: boolean;
  /** Unattended is on: the frame says so. */
  unattended?: boolean;
  /** Above the composer: confirmations and errors. */
  notice?: React.ReactNode;
  /** On the composer's top edge, below the notices: the plan strip. */
  dock?: React.ReactNode;
  /** Inside, above the message: mode warnings. */
  banner?: React.ReactNode;
  leading?: React.ReactNode;
  controls?: React.ReactNode;
  /** Replaces send (the stop button while the agent works). */
  action?: React.ReactNode;
  /** The agent's slash commands, offered when a word starts with `/`. */
  commands?: readonly SessionCommand[] | null;
  /** Where a command the agent does not run itself runs. */
  terminal?: AgentTerminal | null;
  /** `steer`: into the running turn rather than after it. */
  onSend(blocks: PromptBlock[], clientMessageId: string, steer: boolean): Promise<void>;
}) {
  const [text, setText] = useState(props.initialDraft?.text ?? "");
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const sendMode = usePref(sendKey);
  const steerFirst = usePref(followUpMode) === "steer";
  const area = useRef<HTMLTextAreaElement>(null);
  const fileInput = useRef<HTMLInputElement>(null);
  const attachments = usePendingAttachments(props.initialDraft?.files);
  const { draftRef } = props;
  useEffect(() => {
    if (draftRef) draftRef.current = { text, files: attachments.files };
  }, [draftRef, text, attachments.files]);
  const canAttach = attachments.enabled && !props.disabled && !sending;
  const agentTerminal = props.terminal;
  const slash = useSlashMenu({
    text,
    setText,
    area,
    commands: props.commands,
    terminal: agentTerminal && {
      agentName: agentTerminal.agentName,
      run(command) {
        setError(null);
        agentTerminal.run(command).catch((e) => setError(errorMessage(e)));
      },
    },
  });

  // Says `/` opens the menu while it has anything to offer.
  const placeholder =
    !props.disabled && (agentTerminal || props.commands?.length)
      ? `${props.placeholder}, or type / for commands`
      : props.placeholder;

  // Grow with the text up to a limit, then scroll.
  useLayoutEffect(() => {
    const el = area.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, COMPOSER_MAX_PX)}px`;
  }, [text]);

  async function send(steer = !!props.steerable && steerFirst) {
    const trimmed = text.trim();
    const attaching = attachments.files.length > 0;
    if ((!trimmed && !attaching) || sending || props.busy) return;
    setSending(true);
    setError(null);
    try {
      // Files go first; one that does not upload holds the message back.
      const files = attaching ? await attachments.upload(props.workspaceId) : [];
      if (files === null) {
        setError(UPLOAD_FAILED_MESSAGE);
        return;
      }
      // Sent once, tagged so the host's echo of it can be recognised; on an
      // unknown outcome the text stays for the user to decide.
      await props.onSend([...(trimmed ? [{ type: "text" as const, text: trimmed }] : []), ...files], crypto.randomUUID(), steer);
      setText("");
      attachments.clear();
    } catch (e) {
      if (isClientError(e, "attachment_not_found")) attachments.forgetUploads();
      setError(errorMessage(e));
    } finally {
      setSending(false);
    }
  }

  const steering = !!props.steerable && steerFirst;
  const sendLabel = steering ? "Steer" : props.queueing ? "Queue message" : "Send";
  const sendTitle =
    `${sendLabel} (${sendShortcut(sendMode)})` +
    (props.steerable ? ` · ${steering ? "Queue" : "Steer"} (${formatShortcut("Mod+Shift+Enter")})` : "");

  function attach(files: Iterable<File>) {
    if (!canAttach) return;
    setError(null);
    attachments.add(files);
  }

  return (
    <div className="shrink-0 px-6 pt-1 pb-4">
      <div className="mx-auto flex w-full max-w-3xl flex-col gap-2">
        {props.notice}
        {error && (
          <p className="text-destructive text-xs" role="alert">
            {error}
          </p>
        )}
        {props.dock}
        <div
          className={cn(
            "bg-card ease-std relative rounded-2xl shadow-lg ring-1 transition-shadow duration-120",
            props.unattended ? "ring-brand/50" : "ring-border focus-within:ring-ring/60",
          )}
          data-testid="composer"
          onClick={(e) => {
            if (e.target === e.currentTarget) area.current?.focus();
          }}
          onDragOver={(e) => {
            if (canAttach && e.dataTransfer.types.includes("Files")) e.preventDefault();
          }}
          onDrop={(e) => {
            if (!canAttach || e.dataTransfer.files.length === 0) return;
            e.preventDefault();
            attach(e.dataTransfer.files);
          }}
        >
          {slash.menu}
          {props.banner}
          <PendingAttachments
            className="px-3 pt-3"
            files={attachments.files}
            errors={attachments.errors}
            disabled={sending}
            onRemove={attachments.remove}
          />
          <textarea
            ref={area}
            // The composer's ring shows its focus.
            className="placeholder:text-muted-foreground text-body block max-h-60 min-h-12 w-full resize-none bg-transparent px-4 pt-3.5 pb-1 outline-none disabled:cursor-not-allowed"
            placeholder={placeholder}
            value={text}
            rows={1}
            // Read-only while sending, not disabled: the caret stays for the next message.
            readOnly={sending}
            disabled={props.disabled}
            onChange={(e) => {
              setText(e.target.value);
              slash.track(e);
            }}
            onSelect={slash.track}
            onPaste={(e) => {
              if (!canAttach) return;
              const files = pastedFiles(e.clipboardData, autoConvertLongText());
              if (!files) return;
              e.preventDefault();
              attach(files);
            }}
            onKeyDown={(e) => {
              if (slash.onKeyDown(e)) return;
              if (isSendKey(e, sendMode)) {
                e.preventDefault();
                void send();
              } else if (props.steerable && isOtherSendKey(e)) {
                e.preventDefault();
                void send(!steerFirst);
              } else if (canAttach && e.key.toLowerCase() === "u" && (e.metaKey || e.ctrlKey)) {
                e.preventDefault();
                fileInput.current?.click();
              }
            }}
          />
          <input
            ref={fileInput}
            type="file"
            multiple
            hidden
            data-testid="attachment-input"
            onChange={(e) => {
              if (e.target.files) attach(e.target.files);
              e.target.value = "";
            }}
          />
          <div className="flex items-center gap-1 px-2 pb-2">
            {attachments.enabled && (
              <Button
                size="icon-sm"
                variant="ghost"
                title={`Attach files (${formatShortcut("Mod+U")})`}
                aria-label="Attach files"
                disabled={!canAttach || sending}
                onClick={() => fileInput.current?.click()}
              >
                <Paperclip />
              </Button>
            )}
            {props.leading}
            <div className="flex min-w-0 flex-1 items-center justify-end gap-0.5">{props.controls}</div>
            {props.action}
            {(!props.action || props.queueing) && (
              <button
                type="button"
                className="bg-brand text-brand-foreground enabled:hover:bg-brand/88 disabled:bg-muted disabled:text-muted-foreground ease-std grid size-8 shrink-0 place-items-center rounded-full transition-[background-color,color,transform] duration-120 enabled:active:scale-[.96]"
                onClick={() => void send()}
                disabled={props.disabled || props.busy || sending || (!text.trim() && attachments.files.length === 0)}
                title={sendTitle}
                aria-label={sendLabel}
              >
                {sending ? <Loader2 className="size-4 animate-spin" /> : <ArrowUp className="size-4" />}
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
