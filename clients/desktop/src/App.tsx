import { useCallback, useEffect, useRef, useState } from "react";
import {
  ChevronDown,
  ChevronUp,
  FileDiff,
  FileText,
  GitBranch,
  Loader2,
  MessageSquare,
  PanelLeftClose,
  PanelLeftOpen,
  PanelRightClose,
  PanelRightOpen,
  Plus,
  Smartphone,
  WifiOff,
  X,
} from "lucide-react";
import {
  isSessionWorking,
  type ConnectionMode,
  type ConnectionPath,
  type HostController,
  type SessionSummary,
  type WorkspaceInfo,
} from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import { useAccount } from "@/lib/account";
import { bootHost, type BootOptions, type BootPhase } from "@/lib/boot";
import type { NativeBridge } from "@/lib/bridge";
import { errorMessage, HostProvider, useController, useHost, useOnline } from "@/lib/host-context";
import {
  connectionMode,
  hostChoice,
  lastSelection,
  panelLayout,
  PANEL_MAX,
  PANEL_MIN,
  rememberConnectionMode,
  rememberHostChoice,
  rememberPanel,
  rememberSelection,
  rememberSidebar,
  rememberWelcomed,
  sidebarLayout,
  SIDEBAR_MAX,
  SIDEBAR_MIN,
  TERMINAL_MAX,
  TERMINAL_MIN,
  welcomed,
  type HostChoice,
} from "@/lib/prefs";
import { connectRemote } from "@/lib/remote";
import { AccountPanel, Welcome } from "@/panels/Account";
import { cn } from "@/lib/utils";
import { ConnectionStatus, type RemoteLink } from "@/panels/ConnectionStatus";
import { DevicesDialog } from "@/panels/Devices";
import { DiffPanel, DiffView } from "@/panels/DiffPanel";
import { FilesPanel, FileView } from "@/panels/FilesPanel";
import { GitPanel } from "@/panels/GitPanel";
import { HostPathDialog, HostPicker, PairDialog, type PairTarget } from "@/panels/Hosts";
import { SessionPanel } from "@/panels/SessionPanel";
import { Sidebar } from "@/panels/Sidebar";
import { TerminalPanel } from "@/panels/TerminalPanel";

const LOCAL: HostChoice = { kind: "local" };
const sameChoice = (a: HostChoice, b: HostChoice) =>
  a.kind === b.kind && (a.kind === "local" || a.hostId === (b as { hostId: string }).hostId);
const choiceKey = (c: HostChoice) => (c.kind === "local" ? "local" : `remote:${c.hostId}`);

/**
 * A host on another computer, directly or through the account's Cloud as
 * `mode` says. Throws when this computer cannot reach it at all (no way to
 * in this build, not paired): the window then shows this computer's host
 * instead.
 */
async function bootRemote(
  bridge: NativeBridge,
  hostId: string,
  mode: ConnectionMode,
  onPhase: (p: BootPhase) => void,
  onPath: (path: ConnectionPath | null) => void,
): Promise<HostController> {
  onPhase({ phase: "discovering" });
  const remote = bridge.remote;
  const status = await bridge.accountStatus();
  // Local only never contacts the Cloud.
  const cloudUrl = mode === "local" ? null : (status.cloud_url ?? null);
  const lan = mode !== "cloud" && Boolean(remote?.lan);
  if (!remote || (!cloudUrl && !lan)) throw new Error("Remote hosts are not available in this build");
  const { controller, manager } = await connectRemote(remote, cloudUrl, hostId, mode);
  manager.onPath(onPath);
  onPhase({ phase: "ready", controller });
  return controller;
}

export function App({ bridge, bootOptions }: { bridge: NativeBridge; bootOptions?: BootOptions }) {
  const [choice, setChoice] = useState<HostChoice>(() => (bridge.remote ? hostChoice() : LOCAL));
  const [boot, setBoot] = useState<BootPhase>({ phase: "discovering" });
  const [attempt, setAttempt] = useState(0);
  const [mode, setMode] = useState<ConnectionMode>(connectionMode);
  /** The path the remote host's connection takes now. */
  const [path, setPath] = useState<ConnectionPath | null>(null);
  /** The pairing dialog, when open. */
  const [pairing, setPairing] = useState<PairTarget | null>(null);

  /**
   * Show another host. The current connection is closed and its state
   * dropped with it; nothing carries over between hosts.
   */
  const shown = useRef(choice);
  shown.current = choice;
  const choose = useCallback((next: HostChoice) => {
    if (sameChoice(shown.current, next)) return;
    rememberHostChoice(next);
    setBoot({ phase: "discovering" });
    setChoice(next);
  }, []);

  const chooseMode = useCallback((next: ConnectionMode) => {
    rememberConnectionMode(next);
    setMode(next);
  }, []);

  // A new mode reconnects a remote host (the user asked); this computer's
  // host is always reached locally.
  const remoteMode = choice.kind === "remote" ? mode : null;
  useEffect(() => {
    let disposed = false;
    let controller: { dispose(): void } | null = null;
    const onPhase = (p: BootPhase) => {
      if (!disposed) setBoot(p);
    };
    const onPath = (p: ConnectionPath | null) => {
      if (!disposed) setPath(p);
    };
    setPath(null);
    const connecting =
      choice.kind === "local" || !remoteMode
        ? bootHost(bridge, onPhase, bootOptions)
        : bootRemote(bridge, choice.hostId, remoteMode, onPhase, onPath);
    connecting.then(
      (c) => {
        controller = c;
        if (disposed) c?.dispose();
      },
      () => {
        if (!disposed) choose(LOCAL);
      },
    );
    // Closing the window (or switching hosts) only drops this connection;
    // the host keeps running.
    return () => {
      disposed = true;
      controller?.dispose();
    };
  }, [bridge, bootOptions, attempt, choice, choose, remoteMode]);

  const pairDialog = pairing && (
    <PairDialog
      bridge={bridge}
      target={pairing}
      onClose={() => setPairing(null)}
      onPaired={(host) => {
        setPairing(null);
        // Re-pairing the host already shown reconnects it with the new trust.
        if (choice.kind === "remote" && choice.hostId === host.host_id) setAttempt((a) => a + 1);
        else choose({ kind: "remote", hostId: host.host_id });
      }}
    />
  );

  if (boot.phase !== "ready") {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-3" data-testid="boot">
        {boot.phase === "failed" ? (
          <>
            <p className="font-medium">Could not reach the Zwiim host</p>
            <p className="text-muted-foreground max-w-md text-center text-xs">{boot.message}</p>
            <Button size="sm" variant="outline" onClick={() => setAttempt((a) => a + 1)}>
              Try again
            </Button>
            {bridge.remote && (
              <div className="w-64">
                <HostPicker bridge={bridge} mode={mode} choice={choice} onChoose={choose} onPair={setPairing} />
              </div>
            )}
          </>
        ) : (
          <>
            <Loader2 className="text-muted-foreground size-5 animate-spin" />
            <p className="text-muted-foreground">
              {choice.kind === "remote"
                ? "Connecting to the host…"
                : boot.phase === "discovering"
                  ? "Looking for the local host…"
                  : "Starting the host…"}
            </p>
          </>
        )}
        {pairDialog}
      </div>
    );
  }

  return (
    <HostProvider value={boot.controller} key={choiceKey(choice)}>
      <Workbench
        bridge={bridge}
        choice={choice}
        onChoose={choose}
        onPair={setPairing}
        link={{ mode, path, onMode: chooseMode }}
      />
      {pairDialog}
    </HostProvider>
  );
}

type Tab = "files" | "diff" | "git";

const TABS: { id: Tab; label: string; icon: typeof FileText }[] = [
  { id: "files", label: "Files", icon: FileText },
  { id: "diff", label: "Diff", icon: FileDiff },
  { id: "git", label: "Git", icon: GitBranch },
];

/** A file opened beside the conversation: its changes, or its contents. */
interface OpenDoc {
  kind: "diff" | "file";
  workspaceId: string;
  path: string;
}

const docKey = (d: OpenDoc) => `${d.kind}:${d.workspaceId}:${d.path}`;

export function Workbench({
  bridge,
  choice = LOCAL,
  onChoose,
  onPair,
  link,
}: {
  bridge: NativeBridge;
  /** The host shown; this computer's unless one was chosen. */
  choice?: HostChoice;
  onChoose?(choice: HostChoice): void;
  onPair?(target: PairTarget): void;
  /** How a remote host is reached. */
  link?: RemoteLink;
}) {
  const workspaces = useHost((s) => s.workspaces);
  const controller = useController();
  const hostId = useHost((s) => s.host?.host_id ?? null);
  const remote = choice.kind === "remote";
  const refusedCode = useHost((s) => (s.connection.status === "live" ? null : (s.connection.error?.code ?? null)));
  const [showDevices, setShowDevices] = useState(false);
  /** A remote host's folder path being asked for, and where the answer goes. */
  const [askPath, setAskPath] = useState<((path: string | null) => void) | null>(null);
  const [workspaceId, setWorkspaceId] = useState<string | null>(null);
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [docs, setDocs] = useState<OpenDoc[]>([]);
  /** The open file showing in the middle; null shows the session. */
  const [activeDoc, setActiveDoc] = useState<string | null>(null);
  /** Each workspace's session last shown here (null: a new one), to return to it. */
  const lastSession = useRef<Record<string, string | null>>({});
  const restored = useRef(false);
  // Signed out, a remote host cannot be reached through the Cloud: show
  // this computer's. A direct connection never needed the account.
  const direct = link?.mode === "local" || link?.path === "lan";
  const onSignedOut = useCallback(() => {
    if (remote && !direct) onChoose?.(LOCAL);
  }, [remote, direct, onChoose]);
  const account = useAccount(bridge, { remote, onSignedOut });
  const remoteHostId = choice.kind === "remote" ? choice.hostId : null;

  // The host no longer trusts this device (revoked): its pairing is void.
  useEffect(() => {
    if (remoteHostId && refusedCode === "device_refused") void bridge.remote?.forgetHost(remoteHostId);
  }, [bridge, remoteHostId, refusedCode]);
  const [showWelcome, setShowWelcome] = useState(() => !welcomed());
  // First run only: once any account has signed in, signing out means local mode.
  useEffect(() => {
    if (account.account) setShowWelcome(false);
  }, [account.account]);
  const workspace = workspaces?.find((w) => w.workspace_id === workspaceId) ?? null;
  // A removed worktree has no checkout: only its sessions' history remains.
  const removed = workspace?.removed ?? false;
  const shownDocs = workspace && !removed ? docs.filter((d) => d.workspaceId === workspaceId) : [];
  const current = shownDocs.find((d) => docKey(d) === activeDoc) ?? null;

  function selectSession(id: string | null) {
    setSessionId(id);
    setActiveDoc(null);
  }

  /** Show a workspace at the session last shown in it, else its newest one. */
  function selectWorkspace(id: string) {
    if (id === workspaceId) return;
    const sessions = controller.store.getState().sessionsByWorkspace[id] ?? [];
    const last = lastSession.current[id];
    // A removed worktree cannot start a new session, so it opens at its newest.
    const removed = workspaces?.find((w) => w.workspace_id === id)?.removed ?? false;
    const keep = last === null ? !removed : sessions.some((x) => x.session_id === last);
    setWorkspaceId(id);
    setSessionId(keep ? (last ?? null) : (sessions[0]?.session_id ?? null));
    setActiveDoc(null);
  }

  function openDoc(doc: OpenDoc) {
    setDocs((all) => (all.some((d) => docKey(d) === docKey(doc)) ? all : [...all, doc]));
    setActiveDoc(docKey(doc));
  }

  function closeDoc(key: string) {
    const at = shownDocs.findIndex((d) => docKey(d) === key);
    setDocs((all) => all.filter((d) => docKey(d) !== key));
    if (activeDoc === key) {
      const next = shownDocs[at + 1] ?? shownDocs[at - 1];
      setActiveDoc(next ? docKey(next) : null);
    }
  }

  useEffect(() => {
    if (!workspaces || !hostId) return;
    if (!restored.current) {
      // Once per launch: reopen what this client showed last, if the host still has it.
      restored.current = true;
      const last = lastSelection(hostId);
      if (last.workspaceId && workspaces.some((w) => w.workspace_id === last.workspaceId)) {
        const listed = controller.store.getState().sessionsByWorkspace[last.workspaceId] ?? [];
        setWorkspaceId(last.workspaceId);
        setSessionId(listed.some((x) => x.session_id === last.sessionId) ? last.sessionId : null);
        return;
      }
    }
    if (workspaceId && !workspaces.some((w) => w.workspace_id === workspaceId)) {
      setWorkspaceId(null);
      setSessionId(null);
    }
    if (!workspaceId && workspaces.length > 0) setWorkspaceId(workspaces[0]!.workspace_id);
  }, [workspaces, workspaceId, hostId, controller]);

  useEffect(() => {
    if (workspaceId) lastSession.current[workspaceId] = sessionId;
    if (hostId && restored.current) rememberSelection(hostId, { workspaceId, sessionId });
  }, [hostId, workspaceId, sessionId]);

  return (
    <div className="relative flex h-full">
      {showWelcome && account.status?.available && !account.account && (
        <Welcome
          state={account}
          onLocal={() => {
            rememberWelcomed();
            setShowWelcome(false);
          }}
        />
      )}
      <SidebarFrame>
        <div className="flex flex-col gap-2 px-1">
          <span className="text-[15px] font-semibold tracking-tight">Zwiim</span>
          {bridge.remote && onChoose && (
            <HostPicker
              bridge={bridge}
              mode={link?.mode ?? "auto"}
              choice={choice}
              onChoose={onChoose}
              onPair={(t) => onPair?.(t)}
            />
          )}
          <ConnectionStatus
            onStartHost={remote ? undefined : () => bridge.startHost()}
            remote={
              remoteHostId
                ? { onSignIn: account.signIn, onPair: () => onPair?.({ hostId: remoteHostId }) }
                : undefined
            }
            link={remote ? link : undefined}
          />
          <AccountPanel state={account} />
          {!remote && (
            <button
              className="text-muted-foreground hover:text-foreground flex items-center gap-1.5 self-start text-xs"
              onClick={() => setShowDevices(true)}
            >
              <Smartphone className="size-3.5" /> Devices
            </button>
          )}
        </div>
        <Sidebar
          workspaceId={workspaceId}
          onSelectWorkspace={selectWorkspace}
          onSessionStarted={(wid, id) => {
            setWorkspaceId(wid);
            selectSession(id);
          }}
          pickFolder={() =>
            remote ? new Promise<string | null>((resolve) => setAskPath(() => resolve)) : bridge.pickFolder()
          }
        />
      </SidebarFrame>
      {showDevices && <DevicesDialog bridge={bridge} onClose={() => setShowDevices(false)} />}
      {askPath && (
        <HostPathDialog
          onDone={(path) => {
            askPath(path);
            setAskPath(null);
          }}
        />
      )}
      <main className="flex min-w-0 flex-1 flex-col">
        <OfflineBanner />
        {workspaceId ? (
          <>
            {removed && (
              <p
                className="text-muted-foreground border-b px-5 py-1.5 text-xs"
                role="status"
                data-testid="removed-banner"
              >
                This worktree was removed — its sessions are read-only.
              </p>
            )}
            <CenterTabs
              workspaceId={workspaceId}
              sessionId={sessionId}
              removed={removed}
              docs={shownDocs}
              active={current && docKey(current)}
              onSelectSession={selectSession}
              onSelectDoc={setActiveDoc}
              onCloseDoc={closeDoc}
            />
            <div className="relative min-h-0 flex-1">
              <TabPane active={current === null}>
                <SessionPanel workspaceId={workspaceId} sessionId={sessionId} onSessionStarted={setSessionId} />
              </TabPane>
              {workspace &&
                shownDocs.map((d) => (
                  <TabPane key={docKey(d)} active={current === d}>
                    {d.kind === "diff" ? (
                      <DiffView workspace={workspace} path={d.path} />
                    ) : (
                      <FileView workspaceId={d.workspaceId} path={d.path} />
                    )}
                  </TabPane>
                ))}
            </div>
          </>
        ) : (
          <div className="m-auto flex flex-col items-center gap-1.5 text-center">
            <p className="text-lg font-semibold tracking-tight">No workspace yet</p>
            <p className="text-muted-foreground">Add a project folder from the sidebar to get started.</p>
          </div>
        )}
      </main>
      {workspace && !removed && (
        <WorkspacePanel
          workspace={workspace}
          showing={current}
          onOpen={(kind, path) => openDoc({ kind, workspaceId: workspace.workspace_id, path })}
        />
      )}
    </div>
  );
}

const NO_SESSIONS: SessionSummary[] = [];

const tabClass = (selected: boolean) =>
  cn(
    "group -mb-px flex max-w-56 shrink-0 items-center gap-1.5 border-b-2 px-3 text-[13px] transition-colors",
    selected ? "border-foreground text-foreground" : "text-muted-foreground hover:text-foreground border-transparent",
  );

/**
 * The workspace's sessions and the files opened beside them, as tabs over the
 * middle column. "+" starts another session in the same workspace.
 */
function CenterTabs({
  workspaceId,
  sessionId,
  removed,
  docs,
  active,
  onSelectSession,
  onSelectDoc,
  onCloseDoc,
}: {
  workspaceId: string;
  sessionId: string | null;
  removed: boolean;
  docs: OpenDoc[];
  /** The open file showing; null when a session shows. */
  active: string | null;
  onSelectSession(id: string | null): void;
  onSelectDoc(key: string): void;
  onCloseDoc(key: string): void;
}) {
  const listed = useHost((s) => s.sessionsByWorkspace[workspaceId] ?? NO_SESSIONS);
  // The host lists newest first; tabs open left to right.
  const sessions = [...listed].reverse();
  const drafting = sessionId === null && !removed;
  const [deleting, setDeleting] = useState<{ session: SessionSummary; anchor: DOMRect } | null>(null);

  /** After a delete, the tab beside the deleted one shows (or a new session). */
  function deleted(id: string) {
    setDeleting(null);
    if (id !== sessionId) return;
    const at = sessions.findIndex((s) => s.session_id === id);
    onSelectSession((sessions[at + 1] ?? sessions[at - 1])?.session_id ?? null);
  }

  return (
    <>
      <nav className="flex h-11 shrink-0 items-stretch overflow-x-auto border-b px-2" aria-label="Open tabs">
        {sessions.map((s) => (
          <SessionTab
            key={s.session_id}
            session={s}
            selected={active === null && sessionId === s.session_id}
            onSelect={() => onSelectSession(s.session_id)}
            onDelete={(anchor) => setDeleting({ session: s, anchor })}
          />
        ))}
        {drafting && (
          <button
            className={tabClass(active === null)}
            aria-current={active === null ? "page" : undefined}
            onClick={() => onSelectSession(null)}
          >
            <MessageSquare className="size-3.5 shrink-0" />
            <span className="truncate">New session</span>
          </button>
        )}
        {!removed && (
          <button
            className="text-muted-foreground hover:text-foreground hover:bg-muted my-auto grid size-7 shrink-0 place-items-center rounded-md"
            title="New session"
            aria-label="New session"
            onClick={() => onSelectSession(null)}
          >
            <Plus className="size-4" />
          </button>
        )}
        {docs.length > 0 && <span className="bg-border mx-1.5 my-3 w-px shrink-0" aria-hidden />}
        {docs.map((d) => {
          const key = docKey(d);
          const name = d.path.split("/").pop() || d.path;
          const Icon = d.kind === "diff" ? FileDiff : FileText;
          return (
            <div
              key={key}
              className={tabClass(active === key)}
              title={d.kind === "diff" ? `Changes in ${d.path}` : d.path}
              onAuxClick={(e) => e.button === 1 && onCloseDoc(key)}
            >
              <button
                className="flex min-w-0 items-center gap-1.5"
                aria-current={active === key ? "page" : undefined}
                onClick={() => onSelectDoc(key)}
              >
                <Icon className="size-3.5 shrink-0" />
                <span className="truncate">{name}</span>
              </button>
              <button
                className={cn(
                  "hover:bg-muted rounded p-0.5 opacity-0 group-hover:opacity-100 focus-visible:opacity-100",
                  active === key && "opacity-100",
                )}
                aria-label={`Close ${name}`}
                title="Close"
                onClick={() => onCloseDoc(key)}
              >
                <X className="size-3" />
              </button>
            </div>
          );
        })}
      </nav>
      {deleting && (
        <DeleteSession
          session={deleting.session}
          anchor={deleting.anchor}
          onClose={() => setDeleting(null)}
          onDeleted={() => deleted(deleting.session.session_id)}
        />
      )}
    </>
  );
}

/** A session's tab: its title, a dot while its agent works, and delete. */
function SessionTab({
  session,
  selected,
  onSelect,
  onDelete,
}: {
  session: SessionSummary;
  selected: boolean;
  onSelect(): void;
  onDelete(anchor: DOMRect): void;
}) {
  const view = useHost((s) => s.sessions[session.session_id]);
  const working = isSessionWorking(session, view);
  const label = session.title || "Untitled session";
  return (
    <div className={tabClass(selected)} title={working ? `${label} · working` : (session.title ?? undefined)}>
      <button
        className="flex min-w-0 items-center gap-1.5"
        aria-current={selected ? "page" : undefined}
        onClick={onSelect}
      >
        {working ? (
          <span className="grid size-3.5 shrink-0 place-items-center">
            <span className="size-1.5 animate-pulse rounded-full bg-amber-500" />
          </span>
        ) : (
          <MessageSquare className="size-3.5 shrink-0" />
        )}
        <span className="truncate">{label}</span>
      </button>
      <button
        className={cn(
          "hover:bg-muted rounded p-0.5 opacity-0 group-hover:opacity-100 focus-visible:opacity-100",
          selected && "opacity-100",
        )}
        aria-label={`Delete ${label}`}
        title="Delete session"
        onClick={(e) => onDelete(e.currentTarget.getBoundingClientRect())}
      >
        <X className="size-3" />
      </button>
    </div>
  );
}

/**
 * Confirm deleting a session, under its tab. The host refuses while the
 * agent is working; nothing is retried.
 */
function DeleteSession({
  session,
  anchor,
  onClose,
  onDeleted,
}: {
  session: SessionSummary;
  anchor: DOMRect;
  onClose(): void;
  onDeleted(): void;
}) {
  const controller = useController();
  const online = useOnline();
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !pending) onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [pending, onClose]);

  async function remove() {
    setPending(true);
    setError(null);
    try {
      await controller.deleteSession(session.session_id);
      onDeleted();
    } catch (e) {
      const busy = e && typeof e === "object" && "code" in e && e.code === "turn_in_progress";
      setError(busy ? "The agent is still working. Stop it, then delete the session." : errorMessage(e));
      setPending(false);
    }
  }

  return (
    <>
      <div className="fixed inset-0 z-40" onPointerDown={() => !pending && onClose()} />
      <div
        role="alertdialog"
        aria-label="Delete session"
        className="bg-card fixed z-50 flex w-72 flex-col gap-2 rounded-lg border p-3 text-xs shadow-lg"
        style={{ top: anchor.bottom + 6, left: Math.max(8, Math.min(anchor.right - 288, window.innerWidth - 296)) }}
      >
        <p>
          Delete <span className="font-medium">{session.title || "Untitled session"}</span>? Its conversation will no
          longer be available.
        </p>
        {error && <p className="text-destructive">{error}</p>}
        <div className="flex justify-end gap-1.5">
          <Button size="xs" variant="ghost" disabled={pending} onClick={onClose}>
            Cancel
          </Button>
          <Button size="xs" variant="destructive" autoFocus disabled={pending || !online} onClick={() => void remove()}>
            Delete
          </Button>
        </div>
      </div>
    </>
  );
}

/**
 * The workspace beside the conversation: files, changes and history above,
 * its terminals below. Resizable and collapsible; the layout is a client
 * preference.
 */
function WorkspacePanel({
  workspace,
  showing,
  onOpen,
}: {
  workspace: WorkspaceInfo;
  /** The file showing in the middle, to mark it in its list. */
  showing: OpenDoc | null;
  onOpen(kind: OpenDoc["kind"], path: string): void;
}) {
  const [layout, setLayout] = useState(panelLayout);
  const [tab, setTab] = useState<Tab>("files");
  // Views mount on first visit and then stay mounted, so switching tabs
  // keeps each one's state (open files, selected diff, scroll).
  const [visited, setVisited] = useState<Set<Tab>>(() => new Set(["files"]));
  const drag = useRef<{ x: number; width: number } | null>(null);
  const split = useRef<HTMLDivElement>(null);
  const splitting = useRef(false);
  const workspaceId = workspace.workspace_id;

  useEffect(() => rememberPanel(layout), [layout]);

  useEffect(() => {
    setVisited((v) => (v.has(tab) ? v : new Set([...v, tab])));
  }, [tab]);

  // Collapsed, the panel stays mounted so open files and terminals survive.
  return (
    <>
      {layout.collapsed && (
        <aside className="bg-sidebar border-sidebar-border flex shrink-0 flex-col items-center border-l p-1.5">
          <Button
            size="icon-sm"
            variant="ghost"
            title="Show panel"
            onClick={() => setLayout((l) => ({ ...l, collapsed: false }))}
          >
            <PanelRightOpen />
          </Button>
        </aside>
      )}
      <aside
        className={cn(
          "bg-sidebar border-sidebar-border relative flex shrink-0 flex-col border-l",
          layout.collapsed && "hidden",
        )}
        style={{ width: layout.width }}
        aria-hidden={layout.collapsed}
        data-testid="workspace-panel"
      >
        <div
          className="hover:bg-sidebar-border absolute top-0 -left-1 z-10 h-full w-2 cursor-col-resize"
          role="separator"
          aria-orientation="vertical"
          title="Drag to resize"
          onPointerDown={(e) => {
            drag.current = { x: e.clientX, width: layout.width };
            e.currentTarget.setPointerCapture(e.pointerId);
          }}
          onPointerMove={(e) => {
            const d = drag.current;
            if (!d) return;
            const width = Math.min(PANEL_MAX, Math.max(PANEL_MIN, d.width - (e.clientX - d.x)));
            setLayout((l) => ({ ...l, width }));
          }}
          onPointerUp={() => {
            drag.current = null;
          }}
        />
        <nav className="flex h-11 shrink-0 items-center gap-1 border-b px-2" aria-label="Views">
          {TABS.map((t) => (
            <button
              key={t.id}
              className={cn(
                "focus-visible:ring-ring/50 flex h-7 items-center gap-1.5 rounded-md px-2.5 text-[13px] transition-colors outline-none focus-visible:ring-2",
                tab === t.id ? "bg-muted text-foreground" : "text-muted-foreground hover:text-foreground",
              )}
              aria-current={tab === t.id ? "page" : undefined}
              onClick={() => setTab(t.id)}
            >
              <t.icon className="size-3.5" />
              {t.label}
            </button>
          ))}
          <Button
            className="ml-auto"
            size="icon-sm"
            variant="ghost"
            title="Hide panel"
            onClick={() => setLayout((l) => ({ ...l, collapsed: true }))}
          >
            <PanelRightClose />
          </Button>
        </nav>
        <div ref={split} className="flex min-h-0 flex-1 flex-col">
          <div className="relative min-h-0 flex-1">
            <TabPane active={tab === "files"}>
              <FilesPanel
                workspaceId={workspaceId}
                selected={showing?.kind === "file" ? showing.path : null}
                onOpen={(path) => onOpen("file", path)}
              />
            </TabPane>
            {visited.has("diff") && (
              <TabPane active={tab === "diff"}>
                <DiffPanel
                  workspace={workspace}
                  selected={showing?.kind === "diff" ? showing.path : null}
                  onOpen={(path) => onOpen("diff", path)}
                />
              </TabPane>
            )}
            {visited.has("git") && (
              <TabPane active={tab === "git"}>
                <GitPanel workspaceId={workspaceId} />
              </TabPane>
            )}
          </div>
          {layout.terminalCollapsed ? (
            <div className="shrink-0 border-t" />
          ) : (
            <div
              className="hover:bg-sidebar-border h-1 shrink-0 cursor-row-resize border-t"
              role="separator"
              aria-orientation="horizontal"
              title="Drag to resize"
              onPointerDown={(e) => {
                splitting.current = true;
                e.currentTarget.setPointerCapture(e.pointerId);
              }}
              onPointerMove={(e) => {
                const box = split.current?.getBoundingClientRect();
                if (!splitting.current || !box || box.height === 0) return;
                const terminal = Math.min(TERMINAL_MAX, Math.max(TERMINAL_MIN, (box.bottom - e.clientY) / box.height));
                setLayout((l) => ({ ...l, terminal }));
              }}
              onPointerUp={() => {
                splitting.current = false;
              }}
            />
          )}
          <section
            className={cn("min-h-0 shrink-0 overflow-hidden", layout.terminalCollapsed && "h-9")}
            style={layout.terminalCollapsed ? undefined : { height: `${layout.terminal * 100}%` }}
            aria-label="Terminal"
          >
            <TerminalPanel
              workspaceId={workspaceId}
              leading={
                <Button
                  size="icon-sm"
                  variant="ghost"
                  title={layout.terminalCollapsed ? "Show terminal" : "Hide terminal"}
                  onClick={() => setLayout((l) => ({ ...l, terminalCollapsed: !l.terminalCollapsed }))}
                >
                  {layout.terminalCollapsed ? <ChevronUp /> : <ChevronDown />}
                </Button>
              }
            />
          </section>
        </div>
      </aside>
    </>
  );
}

/** A mounted panel; hidden ones keep their state. */
function TabPane({ active, children }: { active: boolean; children: React.ReactNode }) {
  return (
    <div className={cn("absolute inset-0", !active && "invisible")} aria-hidden={!active} data-active={active}>
      {children}
    </div>
  );
}

/** Resizable, collapsible sidebar; its layout is a client preference. */
function SidebarFrame({ children }: { children: React.ReactNode }) {
  const [layout, setLayout] = useState(sidebarLayout);
  const drag = useRef<{ x: number; width: number } | null>(null);

  useEffect(() => rememberSidebar(layout), [layout]);

  if (layout.collapsed) {
    return (
      <aside className="bg-sidebar border-sidebar-border flex shrink-0 flex-col items-center border-r p-1.5">
        <Button
          size="icon-sm"
          variant="ghost"
          title="Show sidebar"
          onClick={() => setLayout((l) => ({ ...l, collapsed: false }))}
        >
          <PanelLeftOpen />
        </Button>
      </aside>
    );
  }
  return (
    <aside
      className="bg-sidebar text-sidebar-foreground border-sidebar-border relative flex shrink-0 flex-col gap-5 border-r px-2.5 py-3"
      style={{ width: layout.width }}
      data-testid="sidebar"
    >
      <Button
        className="absolute top-2.5 right-2"
        size="icon-sm"
        variant="ghost"
        title="Hide sidebar"
        onClick={() => setLayout((l) => ({ ...l, collapsed: true }))}
      >
        <PanelLeftClose />
      </Button>
      {children}
      <div
        className="hover:bg-sidebar-border absolute top-0 -right-1 h-full w-2 cursor-col-resize"
        role="separator"
        aria-orientation="vertical"
        title="Drag to resize"
        onPointerDown={(e) => {
          drag.current = { x: e.clientX, width: layout.width };
          e.currentTarget.setPointerCapture(e.pointerId);
        }}
        onPointerMove={(e) => {
          const d = drag.current;
          if (!d) return;
          const width = Math.min(SIDEBAR_MAX, Math.max(SIDEBAR_MIN, d.width + e.clientX - d.x));
          setLayout((l) => ({ ...l, width }));
        }}
        onPointerUp={() => {
          drag.current = null;
        }}
      />
    </aside>
  );
}

/** While the host is unreachable nothing can change; say so once, up top. */
function OfflineBanner() {
  const status = useHost((s) => s.connection.status);
  if (status === "live") return null;
  const text =
    status === "unauthenticated"
      ? "This app is not authorized by the host. Changes are disabled."
      : status === "error"
        ? "Not connected to the host. Changes are disabled."
        : "Reconnecting to the host… Changes are disabled until it is back.";
  return (
    <div
      className="flex items-center gap-2 border-b bg-amber-500/10 px-4 py-1.5 text-xs text-amber-800 dark:text-amber-300"
      role="status"
      data-testid="offline-banner"
    >
      <WifiOff className="size-3.5" />
      {text}
    </div>
  );
}
