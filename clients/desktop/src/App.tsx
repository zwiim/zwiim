import { useEffect, useRef, useState } from "react";
import {
  FileDiff,
  FileText,
  GitBranch,
  Loader2,
  MessageSquare,
  PanelLeftClose,
  PanelLeftOpen,
  SquareTerminal,
  WifiOff,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { useAccount } from "@/lib/account";
import { bootHost, type BootPhase } from "@/lib/boot";
import type { NativeBridge } from "@/lib/bridge";
import { HostProvider, useController, useHost } from "@/lib/host-context";
import {
  lastSelection,
  rememberSelection,
  rememberSidebar,
  rememberWelcomed,
  sidebarLayout,
  SIDEBAR_MAX,
  SIDEBAR_MIN,
  welcomed,
} from "@/lib/prefs";
import { AccountPanel, Welcome } from "@/panels/Account";
import { cn } from "@/lib/utils";
import { ConnectionStatus } from "@/panels/ConnectionStatus";
import { DiffPanel } from "@/panels/DiffPanel";
import { FilesPanel } from "@/panels/FilesPanel";
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

type Tab = "session" | "files" | "terminal" | "git" | "diff";

const TABS: { id: Tab; label: string; icon: typeof FileText }[] = [
  { id: "session", label: "Session", icon: MessageSquare },
  { id: "files", label: "Files", icon: FileText },
  { id: "terminal", label: "Terminal", icon: SquareTerminal },
  { id: "git", label: "Git", icon: GitBranch },
  { id: "diff", label: "Diff", icon: FileDiff },
];

export function Workbench({ bridge }: { bridge: NativeBridge }) {
  const workspaces = useHost((s) => s.workspaces);
  const controller = useController();
  const hostId = useHost((s) => s.host?.host_id ?? null);
  const [workspaceId, setWorkspaceId] = useState<string | null>(null);
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [tab, setTab] = useState<Tab>("session");
  // Panels mount on first visit and then stay mounted, so switching tabs
  // keeps each one's state (open files, terminal screens, scroll).
  const [visited, setVisited] = useState<Set<Tab>>(() => new Set(["session"]));
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
  const tabs = removed ? TABS.filter((t) => t.id === "session") : TABS;
  const shown: Tab = removed ? "session" : tab;

  useEffect(() => {
    setVisited((v) => (v.has(shown) ? v : new Set([...v, shown])));
  }, [shown]);

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
          <span className="text-base font-semibold tracking-tight">Zwiim</span>
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
            setTab("session");
          }}
          pickFolder={() => bridge.pickFolder()}
        />
      </SidebarFrame>
      <main className="flex min-w-0 flex-1 flex-col">
        <OfflineBanner />
        {workspaceId ? (
          <>
            <nav className="flex items-center gap-1 border-b px-2 py-1">
              {tabs.map((t) => (
                <button
                  key={t.id}
                  className={cn(
                    "flex items-center gap-1.5 rounded-md px-2.5 py-1 text-xs",
                    shown === t.id ? "bg-muted font-medium" : "text-muted-foreground hover:bg-muted/60",
                  )}
                  onClick={() => setTab(t.id)}
                >
                  <t.icon className="size-3.5" />
                  {t.label}
                </button>
              ))}
              {removed && (
                <span className="text-muted-foreground ml-auto text-xs" data-testid="removed-banner">
                  This worktree was removed — its sessions are read-only.
                </span>
              )}
            </nav>
            <div className="relative min-h-0 flex-1">
              <TabPane active={shown === "session"}>
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
              {!removed && visited.has("files") && (
                <TabPane active={shown === "files"}>
                  <FilesPanel workspaceId={workspaceId} />
                </TabPane>
              )}
              {!removed && visited.has("terminal") && (
                <TabPane active={shown === "terminal"}>
                  <TerminalPanel workspaceId={workspaceId} />
                </TabPane>
              )}
              {!removed && visited.has("git") && (
                <TabPane active={shown === "git"}>
                  <GitPanel workspaceId={workspaceId} />
                </TabPane>
              )}
              {!removed && visited.has("diff") && workspace && (
                <TabPane active={shown === "diff"}>
                  <DiffPanel workspace={workspace} />
                </TabPane>
              )}
            </div>
          </>
        ) : (
          <div className="text-muted-foreground m-auto text-center">
            <p>Add a workspace folder to get started.</p>
          </div>
        )}
      </main>
    </div>
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
      className="bg-sidebar text-sidebar-foreground border-sidebar-border relative flex shrink-0 flex-col gap-4 border-r p-3"
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
