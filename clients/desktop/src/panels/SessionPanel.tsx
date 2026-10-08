import { Fragment, useCallback, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { ArrowDown, ArrowUp, GitBranch, Loader2, Paperclip, Square } from "lucide-react";
import {
  effortFor,
  hasCapability,
  isClientError,
  optionUnavailableDetails,
  turnGroups,
  type AgentSummary,
  type AgentType,
  type ChoiceOption,
  type OpParams,
  type OptionsRequest,
  type PromptBlock,
  type SessionCommand,
  type SessionOptions,
  type SessionView,
} from "@zwiim/sdk";
import { PendingAttachments } from "@/attachments/AttachmentChip";
import { UPLOAD_FAILED_MESSAGE, usePendingAttachments } from "@/attachments/usePendingAttachments";
import { Button } from "@/components/ui/button";
import { errorMessage, useController, useHost, useOnline } from "@/lib/host-context";
import { agentChoices, rememberAgentChoices } from "@/lib/prefs";
import { cn } from "@/lib/utils";
import { ContextRing } from "./ContextRing";
import { ControlSelect, OptionsBar, UnattendedBanner } from "./OptionsBar";
import { RequestCard } from "./RequestCard";
import { useSlashMenu } from "./SlashCommands";
import { AgentTerminalCard, AgentTerminalsContext } from "./AgentTerminal";
import { HistoryTurnView, LiveTurnView, stopReasonText, TurnFooter } from "./Transcript";

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
 * the model/effort this client last chose for it (if still offered), and
 * explicit picks. Effort is the chosen model's own (`effortOn`; none on a
 * model without one). Mode always starts at the agent's normal,
 * approval-required mode unless picked.
 */
export function newSessionOptions(
  advertised: SessionOptions,
  remembered: { model: string | null; effort: string | null },
  picks: OptionsRequest,
  effortOn: (model: string | null) => ChoiceOption | null = () => advertised.effort,
): { shown: SessionOptions; request: OptionsRequest } {
  const request: OptionsRequest = {};
  const choose = (key: "model" | "effort", option: ChoiceOption | null) => {
    if (!option) return null;
    // A new session starts from the agent's defaults, so the shown value is
    // always sent — even when it matches what the agent last reported.
    const want = picks[key] ?? remembered[key];
    const current = want && option.values.some((v) => v.value === want) ? want : option.current;
    if (current) request[key] = current;
    return { ...option, current };
  };
  const mode = advertised.mode;
  if (mode && picks.mode && picks.mode !== mode.current && mode.available.includes(picks.mode)) {
    request.mode = picks.mode;
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
 * The agent to preselect: the last one used if still usable, else the first
 * usable supported one, else the first usable one.
 */
export function defaultAgent(agents: AgentSummary[], preferred: string | null): string | null {
  const usable = agents.filter((a) => a.available);
  return (
    usable.find((a) => a.agent === preferred) ??
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
      ? newSessionOptions(advertised, agentChoices(summary.agent), picks, (model) => effortFor(summary, model))
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
}) {
  const removed = useHost((s) => s.workspaces?.find((w) => w.workspace_id === props.workspaceId)?.removed ?? false);
  if (props.sessionId === null) {
    if (removed) {
      return (
        <p className="text-muted-foreground p-6 text-center">
          This worktree was removed. Its sessions stay readable, but no new work can start here.
        </p>
      );
    }
    return <NewSession key={props.workspaceId} workspaceId={props.workspaceId} onStarted={props.onSessionStarted} />;
  }
  return (
    <OpenSession key={props.sessionId} workspaceId={props.workspaceId} sessionId={props.sessionId} removed={removed} />
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
        placeholder="Describe the task"
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
        <button className="underline" onClick={() => void controller.refreshAgents().catch(() => {})}>
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
  removed,
}: {
  workspaceId: string;
  sessionId: string;
  removed: boolean;
}) {
  const controller = useController();
  const online = useOnline();
  const view = useHost((s) => s.sessions[sessionId]);
  const summary = useHost((s) => s.sessionsByWorkspace[workspaceId]?.find((x) => x.session_id === sessionId));
  const agentName = useHost((s) => s.agents?.find((a) => a.agent === summary?.agent)?.name ?? null);
  // Before its agent announces them (or while it is away), the commands it
  // offered last.
  const agentCommands = useHost((s) => s.agents?.find((a) => a.agent === summary?.agent)?.commands ?? null);
  const terminal = useAgentTerminal(workspaceId, removed ? null : summary?.agent);
  const [cancelError, setCancelError] = useState<string | null>(null);
  const [confirmCancel, setConfirmCancel] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  const {
    ref: scroller,
    atBottom,
    jump,
    scrollProps,
  } = useStickToBottom([view?.history, view?.live, view?.settled, view?.pendingRequest, view?.lastError]);

  useEffect(() => {
    controller.openSession(sessionId);
    return () => controller.closeSession(sessionId);
  }, [controller, sessionId]);

  const running = view?.agentStatus === "prompting";
  // A starting agent may be about to run a queued first turn.
  const starting = view?.agentStatus === "connecting";
  useEffect(() => {
    if (!running) setConfirmCancel(false);
  }, [running]);
  const stopped = running ? null : stopReasonText(view?.lastStopReason ?? null);
  const [optionsBusy, setOptionsBusy] = useState(false);
  const [optionsError, setOptionsError] = useState<string | null>(null);
  const history = view?.history;
  const footers = useMemo(() => new Map((history ? turnGroups(history) : []).map((g) => [g.end, g])), [history]);
  // A prompt already in the history while the agent still answers it has no row yet.
  const runningEnd = running && !view?.live.userMessage && history?.length ? history.length - 1 : -1;
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
          className="relative min-h-0 flex-1 overflow-y-auto"
          data-testid="transcript"
        >
          <div className="mx-auto flex w-full max-w-3xl flex-col gap-5 px-6 pt-4 pb-6">
            {view?.history === null && (
              <p className="text-muted-foreground flex items-center gap-1.5 text-xs">
                <Loader2 className="size-3 animate-spin" /> Loading the conversation…
              </p>
            )}
            {empty && <p className="text-muted-foreground py-10 text-center">No messages yet.</p>}
            {view?.history?.map((t, i) => {
              const group = footers.get(i);
              return (
                <Fragment key={t.turn_id}>
                  <HistoryTurnView turn={t} />
                  {group && i !== runningEnd && <TurnFooter group={group} />}
                </Fragment>
              );
            })}
            {view?.settled && <LiveTurnView turn={view.settled} />}
            {view && <LiveTurnView turn={view.live} streaming={running} animate />}
            {stopped && (
              <p className="text-muted-foreground text-xs" data-testid="stop-reason">
                {stopped}
              </p>
            )}
            {view?.lastError && (
              <p className="text-destructive text-xs" role="alert">
                {view.lastError.message}
              </p>
            )}
            {view?.pendingRequest && (
              <RequestCard
                request={view.pendingRequest}
                disabled={!online}
                onAnswer={async (answer) => {
                  await controller.client.request("session.respond", {
                    session_id: sessionId,
                    request_id: view.pendingRequest!.request_id,
                    ...answer,
                  });
                }}
              />
            )}
          </div>
        </div>
        {!atBottom && (
          <Button
            className="bg-card absolute bottom-3 left-1/2 -translate-x-1/2 rounded-full shadow-lg"
            size="xs"
            variant="outline"
            onClick={jump}
          >
            <ArrowDown /> Jump to latest
          </Button>
        )}
      </div>
      <Composer
        workspaceId={workspaceId}
        placeholder={
          removed
            ? "This worktree was removed — the session is read-only"
            : running
              ? "The agent is working…"
              : "Send a message"
        }
        disabled={running || removed || !online}
        unattended={options?.mode?.current === "unattended"}
        commands={view?.commands ?? agentCommands}
        terminal={terminal}
        notice={
          <>
            <AgentTerminalCard workspaceId={workspaceId} />
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
            {running && !removed && confirmCancel && (
              <div
                className="bg-card flex items-center gap-2 rounded-lg px-3 py-2 text-xs shadow-sm"
                data-testid="confirm-cancel"
              >
                <span className="flex-1">Stop the agent's current turn?</span>
                <Button size="xs" variant="ghost" disabled={cancelling} onClick={() => setConfirmCancel(false)}>
                  Keep going
                </Button>
                <Button size="xs" variant="destructive" disabled={!online || cancelling} onClick={cancel}>
                  Stop
                </Button>
              </div>
            )}
          </>
        }
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
              className="bg-foreground text-background hover:bg-foreground/85 focus-visible:ring-ring/50 grid size-8 shrink-0 place-items-center rounded-full outline-none focus-visible:ring-2 disabled:opacity-50"
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
        onSend={(blocks, client_message_id) =>
          controller.sendMessage({ session_id: sessionId, blocks, client_message_id })
        }
      />
    </div>
  );
}

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
        ? "bg-amber-500 animate-pulse"
        : s === "connecting"
          ? "bg-muted-foreground animate-pulse"
          : "bg-emerald-500";
  } else if (view.attach === "attaching") {
    text = "Attaching…";
    dot = "bg-muted-foreground animate-pulse";
  } else if (view.attach === "error") {
    text = view.attachError?.message ?? "Error";
    dot = "bg-destructive";
  } else if (view.attach === "detached") {
    text = "Idle";
    dot = "bg-muted-foreground/60";
  } else {
    return null;
  }
  return (
    <span className="text-muted-foreground flex min-w-0 items-center gap-1.5 text-xs" data-testid="session-status">
      <span className={cn("size-1.5 shrink-0 rounded-full", dot)} />
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

function useStickToBottom(deps: unknown[]) {
  const ref = useRef<HTMLDivElement>(null);
  const pinned = useRef(true);
  // Our own glide is under way: its scrolls are not the reader leaving.
  const gliding = useRef(false);
  const frame = useRef(0);
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
        const target = el.scrollHeight - el.clientHeight;
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
  }, []);

  // The reader takes over: the glide stops where it is.
  const onIntent = useCallback(() => {
    if (gliding.current) stop();
  }, [stop]);

  const jump = useCallback(() => {
    const el = ref.current;
    if (!el) return;
    glide(el, true);
    pinned.current = true;
    setAtBottom(true);
  }, [glide]);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el || !pinned.current) return;
    // A little more to show glides; a conversation just loaded is shown at once.
    const behind = el.scrollHeight - el.scrollTop - el.clientHeight;
    glide(el, behind < el.clientHeight);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);

  return {
    ref,
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
  disabled?: boolean;
  /** Unattended is on: the frame says so. */
  unattended?: boolean;
  /** Above the composer: confirmations and errors. */
  notice?: React.ReactNode;
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
  onSend(blocks: PromptBlock[], clientMessageId: string): Promise<void>;
}) {
  const [text, setText] = useState("");
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const area = useRef<HTMLTextAreaElement>(null);
  const fileInput = useRef<HTMLInputElement>(null);
  const attachments = usePendingAttachments();
  const canAttach = attachments.enabled && !props.disabled;
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

  async function send() {
    const trimmed = text.trim();
    const attaching = attachments.files.length > 0;
    if ((!trimmed && !attaching) || sending) return;
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
      await props.onSend([...(trimmed ? [{ type: "text" as const, text: trimmed }] : []), ...files], crypto.randomUUID());
      setText("");
      attachments.clear();
    } catch (e) {
      if (isClientError(e, "attachment_not_found")) attachments.forgetUploads();
      setError(errorMessage(e));
    } finally {
      setSending(false);
    }
  }

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
        <div
          className={cn(
            "bg-card relative rounded-2xl shadow-lg ring-1 transition-shadow",
            props.unattended ? "ring-amber-500/50" : "ring-border focus-within:ring-ring/60",
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
            className="placeholder:text-muted-foreground block max-h-60 min-h-12 w-full resize-none bg-transparent px-4 pt-3.5 pb-1 leading-relaxed outline-none disabled:cursor-not-allowed"
            placeholder={placeholder}
            value={text}
            rows={1}
            disabled={props.disabled || sending}
            onChange={(e) => {
              setText(e.target.value);
              slash.track(e);
            }}
            onSelect={slash.track}
            onPaste={(e) => {
              if (!canAttach || e.clipboardData.files.length === 0) return;
              e.preventDefault();
              attach(e.clipboardData.files);
            }}
            onKeyDown={(e) => {
              if (slash.onKeyDown(e)) return;
              if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
                e.preventDefault();
                void send();
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
                title="Attach files (⌘U)"
                aria-label="Attach files"
                disabled={!canAttach || sending}
                onClick={() => fileInput.current?.click()}
              >
                <Paperclip />
              </Button>
            )}
            {props.leading}
            <div className="flex min-w-0 flex-1 items-center justify-end gap-0.5">{props.controls}</div>
            {props.action || (
              <button
                type="button"
                className="bg-brand text-brand-foreground hover:bg-brand/85 focus-visible:ring-ring/50 disabled:bg-muted disabled:text-muted-foreground grid size-8 shrink-0 place-items-center rounded-full outline-none focus-visible:ring-2"
                onClick={send}
                disabled={props.disabled || sending || (!text.trim() && attachments.files.length === 0)}
                title="Send"
                aria-label="Send"
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
