import { useEffect, useRef, useState } from "react";
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
  WifiOff,
  X,
} from "lucide-react";
import type { WorkspaceInfo } from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import { useAccount } from "@/lib/account";
import { bootHost, type BootPhase } from "@/lib/boot";
import type { NativeBridge } from "@/lib/bridge";
import { HostProvider, useController, useHost } from "@/lib/host-context";
import {
  lastSelection,
  panelLayout,
  PANEL_MAX,
  PANEL_MIN,
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
} from "@/lib/prefs";
import { AccountPanel, Welcome } from "@/panels/Account";
import { cn } from "@/lib/utils";
import { ConnectionStatus } from "@/panels/ConnectionStatus";
import { DiffPanel, DiffView } from "@/panels/DiffPanel";
import { FilesPanel, FileView } from "@/panels/FilesPanel";
import { GitPanel } from "@/panels/GitPanel";
import { SessionPanel } from "@/panels/SessionPanel";
import { Sidebar } from "@/panels/Sidebar";
import { TerminalPanel } from "@/panels/TerminalPanel";

export function App({ bridge }: { bridge: NativeBridge }) {
  const [boot, setBoot] = useState<BootPhase>({ phase: "discovering" });
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let disposed = false;
    let controller: { dispose(): void } | null = null;
    void bootHost(bridge, (p) => {
      if (!disposed) setBoot(p);
    }).then((c) => {
      controller = c;
      if (disposed) c?.dispose();
    });
    // Closing the window only drops this connection; the host keeps running.
    return () => {
      disposed = true;
      controller?.dispose();
    };
  }, [bridge, attempt]);

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
          </>
        ) : (
          <>
            <Loader2 className="text-muted-foreground size-5 animate-spin" />
            <p className="text-muted-foreground">
              {boot.phase === "discovering" ? "Looking for the local host…" : "Starting the host…"}
            </p>
          </>
        )}
      </div>
    );
  }

  return (
    <HostProvider value={boot.controller}>
      <Workbench bridge={bridge} />
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

export function Workbench({ bridge }: { bridge: NativeBridge }) {
  const workspaces = useHost((s) => s.workspaces);
  const controller = useController();
  const hostId = useHost((s) => s.host?.host_id ?? null);
  const [workspaceId, setWorkspaceId] = useState<string | null>(null);
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [docs, setDocs] = useState<OpenDoc[]>([]);
  /** The open file showing in the middle; null shows the session. */
  const [activeDoc, setActiveDoc] = useState<string | null>(null);
  const restored = useRef(false);
  const account = useAccount(bridge);
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
          <ConnectionStatus onStartHost={() => bridge.startHost()} />
          <AccountPanel state={account} />
        </div>
        <Sidebar
          workspaceId={workspaceId}
          sessionId={sessionId}
          onSelectWorkspace={(id) => {
            setWorkspaceId(id);
            setSessionId(null);
          }}
          onSelectSession={(wid, id) => {
            setWorkspaceId(wid);
            setSessionId(id);
            setActiveDoc(null);
          }}
          pickFolder={() => bridge.pickFolder()}
        />
      </SidebarFrame>
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
            {shownDocs.length > 0 && (
              <DocTabs
                workspaceId={workspaceId}
                sessionId={sessionId}
                docs={shownDocs}
                active={current && docKey(current)}
                onSelect={setActiveDoc}
                onClose={closeDoc}
              />
            )}
            <div className="relative min-h-0 flex-1">
              <TabPane active={current === null}>
                <SessionPanel
                  workspaceId={workspaceId}
                  sessionId={sessionId}
                  onSessionStarted={setSessionId}
                  onWorkspaceCreated={(id) => {
                    setWorkspaceId(id);
                    setSessionId(null);
                  }}
                />
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

/** The session and the files opened beside it, as tabs over the middle column. */
function DocTabs({
  workspaceId,
  sessionId,
  docs,
  active,
  onSelect,
  onClose,
}: {
  workspaceId: string;
  sessionId: string | null;
  docs: OpenDoc[];
  active: string | null;
  onSelect(key: string | null): void;
  onClose(key: string): void;
}) {
  const title = useHost((s) =>
    sessionId ? (s.sessionsByWorkspace[workspaceId]?.find((x) => x.session_id === sessionId)?.title ?? null) : null,
  );
  const tab = (selected: boolean) =>
    cn(
      "group -mb-px flex max-w-56 shrink-0 items-center gap-1.5 border-b-2 px-3 text-[13px] transition-colors",
      selected ? "border-foreground text-foreground" : "text-muted-foreground hover:text-foreground border-transparent",
    );
  return (
    <nav className="flex h-11 shrink-0 items-stretch overflow-x-auto border-b px-2" aria-label="Open tabs">
      <button
        className={tab(active === null)}
        aria-current={active === null ? "page" : undefined}
        onClick={() => onSelect(null)}
      >
        <MessageSquare className="size-3.5 shrink-0" />
        <span className="truncate">{sessionId ? title || "Session" : "New session"}</span>
      </button>
      {docs.map((d) => {
        const key = docKey(d);
        const name = d.path.split("/").pop() || d.path;
        const Icon = d.kind === "diff" ? FileDiff : FileText;
        return (
          <div
            key={key}
            className={tab(active === key)}
            title={d.kind === "diff" ? `Changes in ${d.path}` : d.path}
            onAuxClick={(e) => e.button === 1 && onClose(key)}
          >
            <button
              className="flex min-w-0 items-center gap-1.5"
              aria-current={active === key ? "page" : undefined}
              onClick={() => onSelect(key)}
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
              onClick={() => onClose(key)}
            >
              <X className="size-3" />
            </button>
          </div>
        );
      })}
    </nav>
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
