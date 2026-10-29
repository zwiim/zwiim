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
  WifiOff,
  X,
} from "lucide-react";
import {
  hasCapability,
  isSessionWorking,
  openTarget,
  type ConnectionMode,
  type ConnectionPath,
  type HostController,
  type SessionSummary,
  type WorkspaceInfo,
} from "@zwiim/sdk";
import { FileLinksContext } from "@/components/Markdown";
import { Button } from "@/components/ui/button";
import { useAccount } from "@/lib/account";
import { stepZoom } from "@/lib/appearance";
import { useTurnAlerts } from "@/lib/attention";
import { bootHost, type BootOptions, type BootPhase } from "@/lib/boot";
import type { NativeBridge } from "@/lib/bridge";
import { errorMessage, HostProvider, useController, useHost, useOnline } from "@/lib/host-context";
import { isModKey } from "@/lib/platform";
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
  rememberZoom,
  sidebarLayout,
  SIDEBAR_MAX,
  SIDEBAR_MIN,
  TERMINAL_MAX,
  TERMINAL_MIN,
  welcomed,
  zoom,
  type HostChoice,
} from "@/lib/prefs";
import { usePresence } from "@/lib/presence";
import { connectRemote } from "@/lib/remote";
import { Welcome } from "@/panels/Account";
import { cn } from "@/lib/utils";
import { ConnectionStatus, type RemoteLink } from "@/panels/ConnectionStatus";
import { ControlCenter } from "@/panels/ControlCenter";
import { DiffPanel, DiffView } from "@/panels/DiffPanel";
import { FilesPanel, FileView } from "@/panels/FilesPanel";
import { GitPanel } from "@/panels/GitPanel";
import { HostPathDialog, HostPicker, PairDialog, type PairTarget } from "@/panels/Hosts";
import { PrBanner } from "@/panels/PrBanner";
import { SessionPanel } from "@/panels/SessionPanel";
import { WorkingMark } from "@/components/WorkingMark";
import { SettingsDialog } from "@/panels/Settings";
import { OpenSettingsContext, type SettingsTarget } from "@/settings/open";
import { Sidebar, type ContinueRequest } from "@/panels/Sidebar";
import { TerminalPanel } from "@/panels/TerminalPanel";
import { AgentTerminalsContext, useAgentTerminalsState } from "@/panels/AgentTerminal";
import { UpdateCard, useUpdates } from "@/panels/UpdateCard";

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
  // A current Host shows a root through a worktree, never as the selection (§5.3).
  const publishable = useHost((s) => hasCapability(s.connection.compat, "workspace.publish"));
  const remote = choice.kind === "remote";
  const refusedCode = useHost((s) => (s.connection.status === "live" ? null : (s.connection.error?.code ?? null)));
  /** Where settings are open; null when they are closed. */
  const [settings, setSettings] = useState<SettingsTarget | null>(null);
  const updates = useUpdates(bridge);
  useTurnAlerts(bridge);
  /** A remote host's folder path being asked for, and where the answer goes. */
  const [askPath, setAskPath] = useState<((path: string | null) => void) | null>(null);
  const [workspaceId, setWorkspaceId] = useState<string | null>(null);
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [docs, setDocs] = useState<OpenDoc[]>([]);
  /** The open file showing in the middle; null shows the session. */
  const [activeDoc, setActiveDoc] = useState<string | null>(null);
  /** Slash commands running in an agent's CLI, over each workspace's composer. */
  const agentTerminals = useAgentTerminalsState();
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

  // ⌘, opens settings, as in any Mac app; Mod with = - 0 zooms the window where it can.
  const zoomable = !!bridge.zoom;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === ",") {
        e.preventDefault();
        setSettings((t) => t ?? { tab: "general" });
        return;
      }
      if (!zoomable || !isModKey(e) || e.altKey) return;
      const next =
        e.key === "=" || e.key === "+"
          ? stepZoom(zoom(), 1)
          : e.key === "-" || e.key === "_"
            ? stepZoom(zoom(), -1)
            : e.key === "0"
              ? 1
              : null;
      if (next === null) return;
      e.preventDefault();
      rememberZoom(next);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [zoomable]);

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
  // On a current Host the repository folder's own sessions are read-only history too (§5.3, D8).
  const readOnlyRoot = publishable && workspace?.parent_workspace_id === null;
  const readOnly = removed || readOnlyRoot;
  // On the way back to the host the content stays, dimmed (DESIGN.md "Reconnecting").
  const status = useHost((s) => s.connection.status);
  const reconnecting = status === "reconnecting" || status === "connecting" || status === "handshaking";
  /** A read-only root's Continue, until the sidebar takes it; numbered so each click runs once. */
  const [continueRequest, setContinueRequest] = useState<ContinueRequest | null>(null);
  const continues = useRef(0);
  const shownDocs = workspace && !readOnly ? docs.filter((d) => d.workspaceId === workspaceId) : [];
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
    // A removed worktree or read-only root cannot start a new session, so it opens at its newest.
    const shown = workspaces?.find((w) => w.workspace_id === id);
    const readOnly = (shown?.removed ?? false) || (publishable && shown?.parent_workspace_id === null);
    const keep = last === null ? !readOnly : sessions.some((x) => x.session_id === last);
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
    const state = controller.store.getState();
    /** Show `root`'s `openTarget` worktree as its click would, else nothing. Never creates one. */
    const showTarget = (root: WorkspaceInfo | undefined) => {
      const target = root ? openTarget(state, root.workspace_id) : null;
      setWorkspaceId(target?.workspace_id ?? null);
      setSessionId(target ? (state.sessionsByWorkspace[target.workspace_id]?.[0]?.session_id ?? null) : null);
      setActiveDoc(null);
    };
    /** The first root with a worktree to open (§5.3), else nothing: the empty state. */
    const showFirstTarget = () =>
      showTarget(workspaces.find((w) => w.parent_workspace_id === null && openTarget(state, w.workspace_id)));
    if (!restored.current) {
      // Once per launch: reopen what this client showed last, if the host still has it.
      restored.current = true;
      const last = lastSelection(hostId);
      const remembered = workspaces.find((w) => w.workspace_id === last.workspaceId);
      // On a current Host a remembered root is replaced by its worktree, else nothing.
      if (remembered && publishable && remembered.parent_workspace_id === null) return showTarget(remembered);
      if (remembered) {
        const listed = state.sessionsByWorkspace[remembered.workspace_id] ?? [];
        setWorkspaceId(remembered.workspace_id);
        setSessionId(listed.some((x) => x.session_id === last.sessionId) ? last.sessionId : null);
        return;
      }
      if (publishable) return showFirstTarget();
    }
    if (workspaceId && !workspaces.some((w) => w.workspace_id === workspaceId)) {
      if (publishable) return showFirstTarget();
      setWorkspaceId(null);
      setSessionId(null);
    }
    // A Host from before publishing shows its first workspace, root or not, as it always did.
    if (!publishable && !workspaceId && workspaces.length > 0) setWorkspaceId(workspaces[0]!.workspace_id);
  }, [workspaces, workspaceId, hostId, controller, publishable]);

  useEffect(() => {
    if (workspaceId) lastSession.current[workspaceId] = sessionId;
    if (hostId && restored.current) rememberSelection(hostId, { workspaceId, sessionId });
  }, [hostId, workspaceId, sessionId]);

  const view = (
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
      <SidebarFrame
        // A Continue shows the sidebar, where its dialog or "Preparing workspace…" appears.
        reveal={continueRequest !== null}
        rail={
          <ControlCenter
            compact
            account={account}
            remote={remote}
            updateHint={updates.hint}
            onSettings={(tab) => setSettings({ tab })}
            onCheckUpdates={updates.check}
          />
        }
        footer={
          <>
            <UpdateCard
              bridge={bridge}
              updates={updates}
              remote={remote}
              onSeeChanges={() => setSettings({ tab: "about" })}
            />
            <div className="border-sidebar-border -mx-1.5 border-t px-1.5 pt-2">
              <ControlCenter
                account={account}
                remote={remote}
                updateHint={updates.hint}
                onSettings={(tab) => setSettings({ tab })}
                onCheckUpdates={updates.check}
              />
            </div>
          </>
        }
      >
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
                ? {
                    onSignIn: account.signIn,
                    onPair: () => onPair?.({ hostId: remoteHostId }),
                  }
                : undefined
            }
            link={remote ? link : undefined}
          />
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
          remote={remote}
          continueRequest={continueRequest}
          onContinued={() => setContinueRequest(null)}
        />
      </SidebarFrame>
      {settings && (
        <SettingsDialog
          bridge={bridge}
          remote={remote}
          account={account}
          link={remote ? link : undefined}
          updateHint={updates.hint}
          workspaceId={workspaceId}
          target={settings}
          onTarget={setSettings}
          onClose={() => setSettings(null)}
        />
      )}
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
                This worktree was archived — its sessions are read-only.
              </p>
            )}
            {readOnlyRoot && (
              <p className="text-muted-foreground border-b px-5 py-1.5 text-xs" role="status" data-testid="root-banner">
                These sessions ran in the repository folder — they're read-only.
              </p>
            )}
            <CenterTabs
              workspaceId={workspaceId}
              sessionId={sessionId}
              removed={readOnly}
              docs={shownDocs}
              active={current && docKey(current)}
              onSelectSession={selectSession}
              onSelectDoc={setActiveDoc}
              onCloseDoc={closeDoc}
            />
            <div
              className={cn("relative min-h-0 flex-1 transition-opacity duration-200", reconnecting && "opacity-60")}
            >
              <TabPane active={current === null}>
                {/* A removed worktree has no files to open, nor does a read-only root. */}
                <FileLinksContext.Provider
                  value={
                    workspace && !readOnly
                      ? {
                          workspace,
                          open: (path) =>
                            openDoc({
                              kind: "file",
                              workspaceId: workspace.workspace_id,
                              path,
                            }),
                          // A plain folder has no changes to mark: the file opens as it is.
                          openDiff: (path) =>
                            openDoc({
                              kind: workspace.is_git_repo ? "diff" : "file",
                              workspaceId: workspace.workspace_id,
                              path,
                            }),
                        }
                      : null
                  }
                >
                  <AgentTerminalsContext.Provider value={agentTerminals}>
                    <SessionPanel
                      workspaceId={workspaceId}
                      sessionId={sessionId}
                      onSessionStarted={setSessionId}
                      onContinue={
                        readOnlyRoot
                          ? () =>
                              setContinueRequest({
                                root: workspaceId,
                                nonce: ++continues.current,
                              })
                          : undefined
                      }
                    />
                  </AgentTerminalsContext.Provider>
                </FileLinksContext.Provider>
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
            {publishable && workspaces?.some((w) => w.parent_workspace_id === null) ? (
              <>
                <p className="text-lg font-semibold tracking-tight">No worktree open</p>
                <p className="text-muted-foreground">Pick a repository to open a worktree.</p>
              </>
            ) : (
              <>
                <p className="text-lg font-semibold tracking-tight">No workspace yet</p>
                <p className="text-muted-foreground">Add a project folder from the sidebar to get started.</p>
              </>
            )}
          </div>
        )}
      </main>
      {workspace && !readOnly && (
        <WorkspacePanel
          workspace={workspace}
          sessionId={sessionId}
          hiddenTerminals={agentTerminals.ids}
          showing={current}
          remote={remote}
          onOpen={(kind, path) => openDoc({ kind, workspaceId: workspace.workspace_id, path })}
        />
      )}
    </div>
  );
  return <OpenSettingsContext.Provider value={setSettings}>{view}</OpenSettingsContext.Provider>;
}

const NO_SESSIONS: SessionSummary[] = [];

const tabClass = (selected: boolean) =>
  cn(
    "group -mb-px flex max-w-56 shrink-0 items-center gap-1.5 border-b-2 px-3 text-ui transition-colors",
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
  const [deleting, setDeleting] = useState<{
    session: SessionSummary;
    anchor: DOMRect;
  } | null>(null);
  // The confirm stays mounted through its exit, showing the session it asked about.
  const confirm = usePresence(deleting !== null);
  const lastDeleting = useRef(deleting);
  if (deleting) lastDeleting.current = deleting;
  const confirming = deleting ?? lastDeleting.current;

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
          <Button
            className="text-muted-foreground my-auto shrink-0"
            size="icon-sm"
            variant="ghost"
            title="New session"
            aria-label="New session"
            onClick={() => onSelectSession(null)}
          >
            <Plus />
          </Button>
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
                  "hover:bg-muted rounded-sm p-0.5 opacity-0 transition-[opacity,background-color] group-hover:opacity-100 group-focus-within:opacity-100 focus-visible:opacity-100",
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
      {confirm.mounted && confirming && (
        <DeleteSession
          key={confirming.session.session_id}
          session={confirming.session}
          anchor={confirming.anchor}
          state={confirm.state}
          onClose={() => setDeleting(null)}
          onDeleted={() => deleted(confirming.session.session_id)}
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
        {working ? <WorkingMark /> : <MessageSquare className="size-3.5 shrink-0" />}
        <span className="truncate">{label}</span>
      </button>
      <button
        className={cn(
          "hover:bg-muted rounded-sm p-0.5 opacity-0 transition-[opacity,background-color] group-hover:opacity-100 group-focus-within:opacity-100 focus-visible:opacity-100",
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
  state,
  onClose,
  onDeleted,
}: {
  session: SessionSummary;
  anchor: DOMRect;
  state: "open" | "closed";
  onClose(): void;
  onDeleted(): void;
}) {
  const controller = useController();
  const online = useOnline();
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const open = state === "open";

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !pending) onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, pending, onClose]);

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
      {open && <div className="fixed inset-0 z-40" onPointerDown={() => !pending && onClose()} />}
      <div
        role="alertdialog"
        aria-label="Delete session"
        className="bg-card presence-pop shadow-pop fixed z-50 flex w-72 origin-top-right flex-col gap-2 rounded-lg border p-3 text-xs"
        data-state={state}
        aria-hidden={!open || undefined}
        inert={!open}
        style={{
          top: anchor.bottom + 6,
          left: Math.max(8, Math.min(anchor.right - 288, window.innerWidth - 296)),
        }}
      >
        <p>
          Delete <span className="font-medium">{session.title || "Untitled session"}</span>? Its conversation will no
          longer be available.
        </p>
        {error && <p className="text-destructive">{error}</p>}
        <div className="flex justify-end gap-1.5">
          {/* Focus lands on the safe choice, never the destructive one. */}
          <Button size="xs" variant="ghost" autoFocus disabled={pending} onClick={onClose}>
            Cancel
          </Button>
          <Button size="xs" variant="destructive" disabled={pending || !online} onClick={() => void remove()}>
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
  sessionId,
  hiddenTerminals,
  showing,
  remote,
  onOpen,
}: {
  workspace: WorkspaceInfo;
  /** The chat on screen, which resolves the branch's conflicts. */
  sessionId: string | null;
  /** Terminals shown over the composer, left out of the dock. */
  hiddenTerminals: ReadonlySet<string>;
  /** The file showing in the middle, to mark it in its list. */
  showing: OpenDoc | null;
  /** The host is on another computer, so its `localhost` is not this one's. */
  remote: boolean;
  onOpen(kind: OpenDoc["kind"], path: string): void;
}) {
  const [layout, setLayout] = useState(panelLayout);
  const [tab, setTab] = useState<Tab>("files");
  // Views mount on first visit and then stay mounted, so switching tabs
  // keeps each one's state (open files, selected diff, scroll).
  const [visited, setVisited] = useState<Set<Tab>>(() => new Set(["files"]));
  const drag = useRef<{ x: number; width: number } | null>(null);
  // Width follows the pointer directly while dragging; it animates only on collapse.
  const [resizing, setResizing] = useState(false);
  const split = useRef<HTMLDivElement>(null);
  const splitting = useRef(false);
  // Height follows the pointer directly while dragging; it animates only on collapse.
  const [splitDrag, setSplitDrag] = useState(false);
  const workspaceId = workspace.workspace_id;

  useEffect(() => rememberPanel(layout), [layout]);

  useEffect(() => {
    setVisited((v) => (v.has(tab) ? v : new Set([...v, tab])));
  }, [tab]);

  // Collapsed, the panel narrows to a rail (sliding like the sidebar) and stays
  // mounted, so open files and terminals survive.
  const collapsed = layout.collapsed;
  return (
    <aside
      className={cn(
        "bg-sidebar border-sidebar-border relative shrink-0 border-l",
        !resizing && "ease-std transition-[width] duration-200",
      )}
      style={{ width: collapsed ? RAIL : layout.width }}
    >
      <div className="absolute inset-0 overflow-hidden">
        <div
          className={cn("flex h-full flex-col transition-opacity duration-200", collapsed && "opacity-0")}
          style={{ width: layout.width - 1 }}
          aria-hidden={collapsed}
          inert={collapsed}
          data-testid="workspace-panel"
        >
          <PrBanner workspaceId={workspaceId} sessionId={sessionId} />
          <nav className="flex h-11 shrink-0 items-center gap-1 border-b px-2" aria-label="Views">
            {TABS.map((t) => (
              <button
                key={t.id}
                className={cn(
                  "focus-visible:ring-ring/50 text-ui flex h-7 items-center gap-1.5 rounded-md px-2.5 transition-colors outline-none focus-visible:ring-2",
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
                  <GitPanel workspace={workspace} />
                </TabPane>
              )}
            </div>
            {layout.terminalCollapsed ? (
              <div className="shrink-0 border-t" />
            ) : (
              <div
                className="hover:bg-sidebar-border focus-visible:bg-sidebar-border h-1 shrink-0 cursor-row-resize border-t transition-colors"
                role="separator"
                aria-orientation="horizontal"
                aria-label="Resize terminal"
                aria-valuenow={Math.round(layout.terminal * 100)}
                aria-valuemin={Math.round(TERMINAL_MIN * 100)}
                aria-valuemax={Math.round(TERMINAL_MAX * 100)}
                tabIndex={0}
                title="Drag to resize"
                onKeyDown={(e) => {
                  const step = e.key === "ArrowUp" ? 0.05 : e.key === "ArrowDown" ? -0.05 : 0;
                  if (!step) return;
                  e.preventDefault();
                  setLayout((l) => ({
                    ...l,
                    terminal: Math.min(TERMINAL_MAX, Math.max(TERMINAL_MIN, l.terminal + step)),
                  }));
                }}
                onPointerDown={(e) => {
                  splitting.current = true;
                  setSplitDrag(true);
                  e.currentTarget.setPointerCapture(e.pointerId);
                }}
                onPointerMove={(e) => {
                  const box = split.current?.getBoundingClientRect();
                  if (!splitting.current || !box || box.height === 0) return;
                  const terminal = Math.min(
                    TERMINAL_MAX,
                    Math.max(TERMINAL_MIN, (box.bottom - e.clientY) / box.height),
                  );
                  setLayout((l) => ({ ...l, terminal }));
                }}
                onPointerUp={() => {
                  splitting.current = false;
                  setSplitDrag(false);
                }}
              />
            )}
            <section
              className={cn(
                "min-h-0 shrink-0 overflow-hidden",
                !splitDrag && "ease-std transition-[height] duration-200",
              )}
              // Both heights inline (the tab strip's 2.25rem, or a share of the split) so the change animates.
              style={{
                height: layout.terminalCollapsed ? "2.25rem" : `${layout.terminal * 100}%`,
              }}
              aria-label="Terminal"
            >
              <TerminalPanel
                workspaceId={workspaceId}
                hidden={hiddenTerminals}
                remote={remote}
                onStarted={() => setLayout((l) => (l.terminalCollapsed ? { ...l, terminalCollapsed: false } : l))}
                leading={
                  <Button
                    size="icon-sm"
                    variant="ghost"
                    title={layout.terminalCollapsed ? "Show terminal" : "Hide terminal"}
                    onClick={() =>
                      setLayout((l) => ({
                        ...l,
                        terminalCollapsed: !l.terminalCollapsed,
                      }))
                    }
                  >
                    {layout.terminalCollapsed ? <ChevronUp /> : <ChevronDown />}
                  </Button>
                }
              />
            </section>
          </div>
        </div>
      </div>
      {collapsed ? (
        <Button
          className="animate-fade-in absolute top-1.5 left-1.5"
          size="icon-sm"
          variant="ghost"
          title="Show panel"
          onClick={() => setLayout((l) => ({ ...l, collapsed: false }))}
        >
          <PanelRightOpen />
        </Button>
      ) : (
        <div
          className="hover:bg-sidebar-border focus-visible:bg-sidebar-border absolute top-0 -left-1 z-10 h-full w-2 cursor-col-resize transition-colors"
          role="separator"
          aria-orientation="vertical"
          aria-label="Resize panel"
          aria-valuenow={layout.width}
          aria-valuemin={PANEL_MIN}
          aria-valuemax={PANEL_MAX}
          tabIndex={0}
          title="Drag to resize"
          onKeyDown={(e) => {
            // The panel sits on the right: Left widens it.
            const step = e.key === "ArrowLeft" ? 16 : e.key === "ArrowRight" ? -16 : 0;
            if (!step) return;
            e.preventDefault();
            setLayout((l) => ({
              ...l,
              width: Math.min(PANEL_MAX, Math.max(PANEL_MIN, l.width + step)),
            }));
          }}
          onPointerDown={(e) => {
            drag.current = { x: e.clientX, width: layout.width };
            setResizing(true);
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
            setResizing(false);
          }}
        />
      )}
    </aside>
  );
}

/** A mounted panel; hidden ones keep their state. */
function TabPane({ active, children }: { active: boolean; children: React.ReactNode }) {
  return (
    <div
      className={cn("absolute inset-0", active ? "animate-fade-in" : "invisible")}
      aria-hidden={!active}
      data-active={active}
    >
      {children}
    </div>
  );
}

/** The collapsed sidebar's width: a ghost icon button (28px), its padding and the border. */
const RAIL = 41;

/** Resizable, collapsible sidebar; its layout is a client preference. */
function SidebarFrame({
  children,
  footer,
  rail,
  reveal = false,
}: {
  children: React.ReactNode;
  footer: React.ReactNode;
  /** The collapsed rail's foot (account and settings). */
  rail: React.ReactNode;
  /** Show a collapsed sidebar, for work that happens there. */
  reveal?: boolean;
}) {
  const [layout, setLayout] = useState(sidebarLayout);
  const drag = useRef<{ x: number; width: number } | null>(null);
  // Width follows the pointer directly while dragging; it animates only on collapse.
  const [resizing, setResizing] = useState(false);

  useEffect(() => rememberSidebar(layout), [layout]);
  useEffect(() => {
    if (reveal) setLayout((l) => (l.collapsed ? { ...l, collapsed: false } : l));
  }, [reveal]);

  const collapsed = layout.collapsed;
  // Collapsed, the sidebar narrows to a rail and stays mounted, so its state survives.
  return (
    <aside
      className={cn(
        "bg-sidebar text-sidebar-foreground border-sidebar-border relative shrink-0 border-r",
        !resizing && "ease-std transition-[width] duration-200",
      )}
      style={{ width: collapsed ? RAIL : layout.width }}
    >
      <div className="absolute inset-0 overflow-hidden">
        <div
          className={cn(
            "relative flex h-full flex-col gap-5 px-2.5 py-3 transition-opacity duration-200",
            collapsed && "opacity-0",
          )}
          style={{ width: layout.width - 1 }}
          aria-hidden={collapsed || undefined}
          inert={collapsed}
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
          <div className="-mx-2.5 -mb-1 flex flex-col px-1.5">{footer}</div>
        </div>
      </div>
      {collapsed ? (
        <>
          <Button
            className="animate-fade-in absolute top-1.5 left-1.5"
            size="icon-sm"
            variant="ghost"
            title="Show sidebar"
            onClick={() => setLayout((l) => ({ ...l, collapsed: false }))}
          >
            <PanelLeftOpen />
          </Button>
          <div className="animate-fade-in absolute bottom-2 left-1.5">{rail}</div>
        </>
      ) : (
        <div
          className="hover:bg-sidebar-border focus-visible:bg-sidebar-border absolute top-0 -right-1 z-10 h-full w-2 cursor-col-resize transition-colors"
          role="separator"
          aria-orientation="vertical"
          aria-label="Resize sidebar"
          aria-valuenow={layout.width}
          aria-valuemin={SIDEBAR_MIN}
          aria-valuemax={SIDEBAR_MAX}
          tabIndex={0}
          title="Drag to resize"
          onKeyDown={(e) => {
            const step = e.key === "ArrowRight" ? 16 : e.key === "ArrowLeft" ? -16 : 0;
            if (!step) return;
            e.preventDefault();
            setLayout((l) => ({
              ...l,
              width: Math.min(SIDEBAR_MAX, Math.max(SIDEBAR_MIN, l.width + step)),
            }));
          }}
          onPointerDown={(e) => {
            drag.current = { x: e.clientX, width: layout.width };
            setResizing(true);
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
            setResizing(false);
          }}
        />
      )}
    </aside>
  );
}

/** While the host is unreachable nothing can change; say so once, up top, quietly. */
function OfflineBanner() {
  const status = useHost((s) => s.connection.status);
  const offline = status !== "live";
  const { mounted, state } = usePresence(offline, 200);
  // Leaving, the banner keeps saying what it said.
  const last = useRef(status);
  if (offline) last.current = status;
  if (!mounted) return null;
  const shown = last.current;
  const text =
    shown === "incompatible"
      ? "An update is required before this app can use the host. Changes are disabled."
      : shown === "unauthenticated"
        ? "This app is not authorized by the host. Changes are disabled."
        : shown === "error"
          ? "Not connected to the host. Changes are disabled."
          : "Reconnecting to the host… Changes are disabled until it is back.";
  const failed = shown === "incompatible" || shown === "unauthenticated" || shown === "error";
  return (
    <div className="collapsible shrink-0" data-state={state}>
      <div>
        <div
          className="text-muted-foreground animate-fade-in flex items-center gap-2 border-b px-4 py-1.5 text-xs"
          role="status"
          data-testid="offline-banner"
        >
          {failed ? (
            <WifiOff className="size-3.5" />
          ) : (
            <span className="grid size-3.5 place-items-center">
              <span className="bg-muted-foreground animate-breathe size-2 rounded-full" />
            </span>
          )}
          {text}
        </div>
      </div>
    </div>
  );
}
