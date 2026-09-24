import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { ArrowDown, Loader2, Square, SendHorizontal } from "lucide-react";
import {
  optionUnavailableDetails,
  type AgentSummary,
  type OptionsRequest,
  type PromptBlock,
  type SessionOptions,
  type SessionView,
} from "@zwiim/sdk";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { errorMessage, useController, useHost, useOnline } from "@/lib/host-context";
import { agentChoices, rememberAgentChoices } from "@/lib/prefs";
import { OptionsBar } from "./OptionsBar";
import { RequestCard } from "./RequestCard";
import { HistoryTurnView, LiveTurnView, stopReasonText } from "./Transcript";

const UNAVAILABLE: Record<string, string> = {
  not_installed: "not installed",
  unsupported_platform: "not supported here",
  disabled: "disabled",
};

/** "Codex — not installed" */
export function agentLabel(a: AgentSummary): string {
  return a.available ? a.name : `${a.name} — ${UNAVAILABLE[a.reason ?? ""] ?? "unavailable"}`;
}

/**
 * What a new session of an agent starts with: the agent's advertised options,
 * the model/effort this client last chose for it (if still offered), and
 * explicit picks. Mode always starts at the agent's normal, approval-required
 * mode unless picked.
 */
export function newSessionOptions(
  advertised: SessionOptions,
  remembered: { model: string | null; effort: string | null },
  picks: OptionsRequest,
): { shown: SessionOptions; request: OptionsRequest } {
  const request: OptionsRequest = {};
  const choose = (key: "model" | "effort") => {
    const option = advertised[key];
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
  return {
    shown: {
      model: choose("model"),
      effort: choose("effort"),
      mode: mode && request.mode ? { ...mode, current: request.mode, label: "" } : mode,
    },
    request,
  };
}

/** The agent to preselect: the last one used if still usable, else the first usable one. */
export function defaultAgent(agents: AgentSummary[], preferred: string | null): string | null {
  const usable = agents.filter((a) => a.available);
  return (usable.find((a) => a.agent === preferred) ?? usable[0])?.agent ?? null;
}

export function SessionPanel(props: {
  workspaceId: string;
  sessionId: string | null;
  onSessionStarted(id: string): void;
  /** "New worktree + session" selected the worktree it created. */
  onWorkspaceCreated?(id: string): void;
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
    return (
      <NewSession
        workspaceId={props.workspaceId}
        onStarted={props.onSessionStarted}
        onWorkspaceCreated={props.onWorkspaceCreated}
      />
    );
  }
  return (
    <OpenSession
      key={props.sessionId}
      workspaceId={props.workspaceId}
      sessionId={props.sessionId}
      removed={removed}
    />
  );
}

function NewSession({
  workspaceId,
  onStarted,
  onWorkspaceCreated,
}: {
  workspaceId: string;
  onStarted(id: string): void;
  onWorkspaceCreated?(id: string): void;
}) {
  const controller = useController();
  const online = useOnline();
  const agents = useHost((s) => s.agents);
  const [chosen, setChosen] = useState<string | null>(null);
  const agent = agents ? defaultAgent(agents, chosen) : null;
  const advertised = agents?.find((a) => a.agent === agent)?.options ?? null;
  const [picks, setPicks] = useState<OptionsRequest>({});
  const start = advertised && agent ? newSessionOptions(advertised, agentChoices(agent), picks) : null;
  const isRoot = useHost((s) => s.workspaces?.find((w) => w.workspace_id === workspaceId)?.kind === "root");
  const [inWorktree, setInWorktree] = useState(false);
  return (
    <div className="flex h-full flex-col">
      <div className="flex flex-1 flex-col items-center justify-center gap-3 p-6">
        <p className="text-lg font-medium">Start a session</p>
        {agents === null ? (
          <p className="text-muted-foreground text-xs">Checking which agents this host can run…</p>
        ) : agent === null ? (
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
        ) : (
          <select
            className="bg-background rounded-md border px-2.5 py-1.5"
            value={agent}
            onChange={(e) => {
              setChosen(e.target.value);
              setPicks({});
            }}
            aria-label="Agent"
          >
            {agents.map((a) => (
              <option key={a.agent} value={a.agent} disabled={!a.available}>
                {agentLabel(a)}
              </option>
            ))}
          </select>
        )}
        {start && (
          <OptionsBar
            options={start.shown}
            disabled={!online}
            onChange={(req) => {
              setPicks((p) => ({ ...p, ...req }));
              if (agent) rememberAgentChoices(agent, { model: req.model, effort: req.effort });
            }}
          />
        )}
        {isRoot && (
          <label className="text-muted-foreground flex items-center gap-1.5 text-xs">
            <input type="checkbox" checked={inWorktree} onChange={(e) => setInWorktree(e.target.checked)} />
            In a new worktree
          </label>
        )}
      </div>
      <Composer
        placeholder="Describe the task…"
        disabled={!online || agent === null}
        onSend={async (blocks, client_message_id) => {
          if (agent === null) return;
          // "New worktree + session" is two host operations: create, then start
          // in it. If the start fails, the worktree stays selected for a retry.
          let target = workspaceId;
          if (isRoot && inWorktree) {
            target = (await controller.createWorktree(workspaceId)).workspace_id;
            setInWorktree(false);
            onWorkspaceCreated?.(target);
          }
          const options = start?.request ?? {};
          const id = await controller.startSession({
            workspace_id: target,
            agent,
            blocks,
            client_message_id,
            ...(Object.keys(options).length > 0 ? { options } : {}),
          });
          onStarted(id);
        }}
      />
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
  const [cancelError, setCancelError] = useState<string | null>(null);
  const [confirmCancel, setConfirmCancel] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  const { ref: scroller, atBottom, onScroll, jump } = useStickToBottom([
    view?.history,
    view?.live,
    view?.settled,
    view?.pendingRequest,
    view?.lastError,
  ]);

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

  async function changeOptions(request: OptionsRequest) {
    setOptionsBusy(true);
    setOptionsError(null);
    try {
      await controller.setSessionOptions(sessionId, request);
      if (summary?.agent) rememberAgentChoices(summary.agent, { model: request.model, effort: request.effort });
    } catch (e) {
      // The host changed nothing; the bar keeps showing what the agent holds.
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

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex items-center gap-2 border-b px-4 py-2">
        <span className="truncate font-medium">{summary?.title || "Session"}</span>
        {summary?.agent && <Badge variant="outline">{summary.agent}</Badge>}
        <SessionBadge view={view} />
        <div className="ml-auto flex items-center gap-2">
          {cancelError && <span className="text-destructive text-xs">{cancelError}</span>}
          {running && !removed && !confirmCancel && (
            <Button size="xs" variant="outline" disabled={!online} onClick={() => setConfirmCancel(true)}>
              <Square /> Cancel
            </Button>
          )}
          {running && !removed && confirmCancel && (
            <span className="flex items-center gap-1.5 text-xs" data-testid="confirm-cancel">
              Stop the agent's current turn?
              <Button size="xs" variant="destructive" disabled={!online || cancelling} onClick={cancel}>
                Stop
              </Button>
              <Button size="xs" variant="ghost" disabled={cancelling} onClick={() => setConfirmCancel(false)}>
                Keep going
              </Button>
            </span>
          )}
        </div>
      </div>
      {view?.options && !removed && (
        <div className="border-b px-4 py-2">
          <OptionsBar
            options={view.options}
            disabled={running || starting || !online || optionsBusy}
            disabledReason={running ? "Options change between turns" : starting ? "The agent is starting…" : null}
            onChange={(req) => void changeOptions(req)}
          />
          {optionsError && (
            <p className="text-destructive mt-1 text-xs" role="alert" data-testid="options-error">
              {optionsError}
            </p>
          )}
        </div>
      )}
      <div className="relative flex min-h-0 flex-1 flex-col">
      <div
        ref={scroller}
        onScroll={onScroll}
        className="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto px-4 py-4"
        data-testid="transcript"
      >
        {view?.history === null && (
          <p className="text-muted-foreground flex items-center gap-1.5 text-xs">
            <Loader2 className="size-3 animate-spin" /> Loading the conversation…
          </p>
        )}
        {view?.history?.map((t) => <HistoryTurnView key={t.turn_id} turn={t} />)}
        {view?.settled && <LiveTurnView turn={view.settled} />}
        {view && <LiveTurnView turn={view.live} streaming={running} />}
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
      {!atBottom && (
        <Button
          className="absolute bottom-3 left-1/2 -translate-x-1/2 shadow-md"
          size="xs"
          variant="outline"
          onClick={jump}
        >
          <ArrowDown /> Jump to latest
        </Button>
      )}
      </div>
      <Composer
        placeholder={
          removed
            ? "This worktree was removed — the session is read-only"
            : running
              ? "The agent is working…"
              : "Send a message"
        }
        disabled={running || removed || !online}
        onSend={(blocks, client_message_id) => controller.sendMessage({ session_id: sessionId, blocks, client_message_id })}
      />
    </div>
  );
}

function SessionBadge({ view }: { view: SessionView | undefined }) {
  if (!view) return null;
  if (view.attach === "live") {
    return (
      <Badge variant={view.agentStatus === "prompting" ? "warning" : view.agentStatus === "connecting" ? "secondary" : "success"}>
        {view.agentStatus === "prompting" ? "Working" : view.agentStatus === "connecting" ? "Starting…" : "Agent ready"}
      </Badge>
    );
  }
  if (view.attach === "attaching") return <Badge>Attaching…</Badge>;
  if (view.attach === "error") return <Badge variant="destructive">{view.attachError?.message ?? "Error"}</Badge>;
  if (view.attach === "detached") return <Badge variant="outline">Idle</Badge>;
  return null;
}

/**
 * Follow new content while the reader is at the bottom; once they scroll up,
 * stay put and offer a jump back.
 */
function useStickToBottom(deps: unknown[]) {
  const ref = useRef<HTMLDivElement>(null);
  const pinned = useRef(true);
  const [atBottom, setAtBottom] = useState(true);

  const onScroll = useCallback(() => {
    const el = ref.current;
    if (!el) return;
    const bottom = el.scrollHeight - el.scrollTop - el.clientHeight < 48;
    pinned.current = bottom;
    setAtBottom(bottom);
  }, []);

  const jump = useCallback(() => {
    const el = ref.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
    pinned.current = true;
    setAtBottom(true);
  }, []);

  useLayoutEffect(() => {
    const el = ref.current;
    if (el && pinned.current) el.scrollTop = el.scrollHeight;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);

  return { ref, atBottom, onScroll, jump };
}

const COMPOSER_MAX_PX = 192;

function Composer(props: {
  placeholder: string;
  disabled?: boolean;
  onSend(blocks: PromptBlock[], clientMessageId: string): Promise<void>;
}) {
  const [text, setText] = useState("");
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const area = useRef<HTMLTextAreaElement>(null);

  // Grow with the text up to a limit, then scroll.
  useLayoutEffect(() => {
    const el = area.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, COMPOSER_MAX_PX)}px`;
  }, [text]);

  async function send() {
    const trimmed = text.trim();
    if (!trimmed || sending) return;
    setSending(true);
    setError(null);
    try {
      // Sent once, tagged so the host's echo of it can be recognised; on an
      // unknown outcome the text stays for the user to decide.
      await props.onSend([{ type: "text", text: trimmed }], crypto.randomUUID());
      setText("");
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setSending(false);
    }
  }

  return (
    <div className="border-t p-3">
      {error && (
        <p className="text-destructive mb-2 text-xs" role="alert">
          {error}
        </p>
      )}
      <div className="flex items-end gap-2">
        <textarea
          ref={area}
          className="bg-background focus-visible:ring-ring/50 max-h-48 min-h-10 flex-1 resize-none rounded-md border px-3 py-2 outline-none focus-visible:ring-[3px]"
          placeholder={props.placeholder}
          value={text}
          rows={1}
          disabled={props.disabled || sending}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
              e.preventDefault();
              void send();
            }
          }}
        />
        <Button size="icon" onClick={send} disabled={props.disabled || sending || !text.trim()} title="Send">
          {sending ? <Loader2 className="animate-spin" /> : <SendHorizontal />}
        </Button>
      </div>
    </div>
  );
}
