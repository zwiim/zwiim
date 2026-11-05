import { useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { ChevronDown, ExternalLink, Play, Plus, RotateCcw, Settings, Square, SquareTerminal, X } from "lucide-react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import {
  DEFAULT_SCROLLBACK_BYTES,
  hasCapability,
  SCRIPT_TERMINAL_TITLES,
  ZwiimClientError,
  type HostController,
  type Preview,
  type RepositoryScript,
} from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import { TERMINAL_BG, TERMINAL_FG, terminalFonts } from "@/lib/appearance";
import { openLink } from "@/lib/bridge";
import { errorMessage, useController, useHost, useOnline } from "@/lib/host-context";
import { fonts } from "@/lib/prefs";
import { usePresence } from "@/lib/presence";
import { usePref } from "@/lib/use-pref";
import { cn } from "@/lib/utils";
import { useOpenSettings } from "@/settings/open";
import { focusMenu, moveMenuFocus } from "./ControlCenter";

/**
 * Sends keystrokes one request at a time so they reach the PTY in order,
 * coalescing what is typed while a write is in flight. Never retries: a
 * failed write drops its bytes and reports the error.
 */
export class InputQueue {
  private buffer = "";
  private inFlight = false;
  constructor(
    private readonly send: (data: string) => Promise<unknown>,
    private readonly onError: (e: unknown) => void,
  ) {}

  push(data: string): void {
    this.buffer += data;
    void this.pump();
  }

  private async pump(): Promise<void> {
    if (this.inFlight || !this.buffer) return;
    const data = this.buffer;
    this.buffer = "";
    this.inFlight = true;
    try {
      await this.send(data);
    } catch (e) {
      this.buffer = "";
      this.onError(e);
    } finally {
      this.inFlight = false;
    }
    void this.pump();
  }
}

/** A terminal tab's kill/close button. */
const CLOSE =
  "text-muted-foreground hover:text-foreground flex size-4 items-center justify-center rounded-sm transition-colors disabled:opacity-50";

export function TerminalPanel({
  workspaceId,
  leading,
  hidden,
  remote = false,
  onStarted,
}: {
  workspaceId: string;
  /** Rendered before the terminal tabs (e.g. a collapse toggle). */
  leading?: React.ReactNode;
  /** Terminals shown elsewhere (a slash command over the composer). */
  hidden?: ReadonlySet<string>;
  /** The host is on another computer: the servers it runs are not on this one. */
  remote?: boolean;
  /** A terminal was started for the user to watch (e.g. so a collapsed panel opens). */
  onStarted?: () => void;
}) {
  const controller = useController();
  const online = useOnline();
  const terminals = useHost((s) => s.terminals);
  const [active, setActive] = useState<string | null>(null);
  // Only a terminal the user opened or picked takes focus, not one restored
  // on launch while they may be typing a message.
  const [focusId, setFocusId] = useState<string | null>(null);
  const [confirmKill, setConfirmKill] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Repository scripts run in a worktree, by a Host that can.
  const scripts = useHost(
    (s) =>
      hasCapability(s.connection.compat, "workspace.run_script") &&
      (s.workspaces ?? []).some((w) => w.workspace_id === workspaceId && w.kind === "worktree"),
  );
  const [starting, setStarting] = useState<RepositoryScript | null>(null);
  const [unset, setUnset] = useState<RepositoryScript | null>(null);
  // Its servers open on the Host's computer, so only when this is it.
  const previews =
    useHost((s) => scripts && hasCapability(s.connection.compat, "workspace.previews")) && !remote;
  const [noPreview, setNoPreview] = useState(false);
  const openSettings = useOpenSettings();
  // Only this workspace's terminals; others belong to their own workspaces.
  const list = useMemo(
    () => Object.values(terminals).filter((t) => t.workspace_id === workspaceId && !hidden?.has(t.terminal_id)),
    [terminals, workspaceId, hidden],
  );

  useEffect(() => {
    if (!active || !list.some((t) => t.terminal_id === active)) setActive(list[0]?.terminal_id ?? null);
    if (confirmKill && !list.some((t) => t.terminal_id === confirmKill && t.alive)) setConfirmKill(null);
  }, [active, confirmKill, list]);

  const running = list.find((t) => t.alive && t.title === SCRIPT_TERMINAL_TITLES.run);

  useEffect(() => {
    setUnset(null);
    setNoPreview(false);
  }, [workspaceId]);

  async function spawn() {
    setError(null);
    try {
      const id = await controller.spawnTerminal(workspaceId);
      setActive(id);
      setFocusId(id);
      onStarted?.();
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  async function runScript(script: RepositoryScript) {
    setError(null);
    setUnset(null);
    setStarting(script);
    try {
      const id = await controller.runScript(workspaceId, script);
      // Shown, not focused: a script is watched, not typed in.
      setActive(id);
      onStarted?.();
    } catch (e) {
      if (e instanceof ZwiimClientError && e.code === "script_not_set") setUnset(script);
      else setError(errorMessage(e));
    } finally {
      setStarting(null);
    }
  }

  async function kill(id: string) {
    setConfirmKill(null);
    setError(null);
    try {
      await controller.client.request("terminal.kill", { terminal_id: id });
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  const labelOf = (id: string) => {
    const i = list.findIndex((t) => t.terminal_id === id);
    return list[i]?.title || `Terminal ${i + 1}`;
  };

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div
        className="flex h-9 shrink-0 items-center gap-1 overflow-x-auto border-b px-2"
        role="tablist"
        aria-label="Terminals"
      >
        {leading}
        {list.map((t, i) => (
          <div
            key={t.terminal_id}
            className={cn(
              "flex shrink-0 items-center gap-1 rounded-md px-2 py-0.5 text-xs transition-colors",
              active === t.terminal_id ? "bg-accent font-medium" : "hover:bg-muted",
              !t.alive && "text-muted-foreground",
            )}
          >
            <button
              role="tab"
              aria-selected={active === t.terminal_id}
              title={t.alive ? undefined : "This terminal has exited"}
              onClick={() => {
                setActive(t.terminal_id);
                setFocusId(t.terminal_id);
              }}
            >
              {t.title || `Terminal ${i + 1}`}
              {!t.alive && " (exited)"}
            </button>
            {t.alive ? (
              <button
                className={CLOSE}
                title="Kill terminal"
                aria-label={`Kill ${t.title || `Terminal ${i + 1}`}`}
                disabled={!online}
                onClick={() => setConfirmKill(t.terminal_id)}
              >
                <X className="size-3" />
              </button>
            ) : (
              <button
                className={CLOSE}
                title="Close"
                aria-label="Close"
                onClick={() => controller.detachTerminal(t.terminal_id)}
              >
                <X className="size-3" />
              </button>
            )}
          </div>
        ))}
        <Button size="icon-sm" variant="ghost" title="New terminal" onClick={spawn} disabled={!online}>
          <Plus />
        </Button>
        {error && <span className="text-destructive ml-2 text-xs">{error}</span>}
        {scripts && (
          <div className="ml-auto flex shrink-0 items-center gap-1">
            {previews && (
              <OpenPreview
                workspaceId={workspaceId}
                onNone={() => setNoPreview(true)}
                onError={(e) => setError(e)}
                onSettings={
                  openSettings && (() => openSettings({ tab: "repository", settingId: "preview-urls" }))
                }
              />
            )}
            <Button
              size="icon-sm"
              variant="ghost"
              title="Run setup again"
              disabled={!online || starting !== null}
              onClick={() => runScript("setup")}
            >
              <RotateCcw />
            </Button>
            {running ? (
              <Button
                size="xs"
                variant="ghost"
                title="Stop the run script"
                disabled={!online}
                onClick={() => kill(running.terminal_id)}
              >
                <Square className="fill-current" />
                Stop
              </Button>
            ) : (
              <Button
                size="xs"
                variant="ghost"
                title="Start the repository's run script"
                disabled={!online || starting !== null}
                onClick={() => runScript("run")}
              >
                <Play className="fill-current" />
                Run
              </Button>
            )}
          </div>
        )}
      </div>
      {unset && (
        <div className="flex items-center gap-2 border-b px-3 py-1.5 text-xs" role="status">
          <span className="flex-1">This repository has no {unset} script yet.</span>
          {openSettings && (
            <Button
              size="xs"
              variant="outline"
              onClick={() => {
                setUnset(null);
                openSettings({ tab: "repository", settingId: `script-${unset}` });
              }}
            >
              Add one in Settings
            </Button>
          )}
          <Button size="xs" variant="ghost" onClick={() => setUnset(null)}>
            Dismiss
          </Button>
        </div>
      )}
      {noPreview && (
        <div className="flex items-center gap-2 border-b px-3 py-1.5 text-xs" role="status">
          <span className="flex-1">
            Nothing to open yet. Start a server with Run, or add this repository's preview URLs.
          </span>
          {openSettings && (
            <Button
              size="xs"
              variant="outline"
              onClick={() => {
                setNoPreview(false);
                openSettings({ tab: "repository", settingId: "preview-urls" });
              }}
            >
              Add one in Settings
            </Button>
          )}
          <Button size="xs" variant="ghost" onClick={() => setNoPreview(false)}>
            Dismiss
          </Button>
        </div>
      )}
      {confirmKill && (
        <div
          className="bg-destructive/5 flex items-center gap-2 border-b px-3 py-1.5 text-xs"
          data-testid="confirm-kill"
        >
          <span className="flex-1">Kill {labelOf(confirmKill)}? Anything running in it stops.</span>
          <Button size="xs" variant="destructive" disabled={!online} onClick={() => kill(confirmKill)}>
            Kill
          </Button>
          <Button size="xs" variant="ghost" onClick={() => setConfirmKill(null)}>
            Keep
          </Button>
        </div>
      )}
      <div className={cn("min-h-0 flex-1 overflow-hidden p-1", active && "bg-terminal")}>
        {active ? (
          <TerminalView
            key={active}
            controller={controller}
            terminalId={active}
            alive={list.find((t) => t.terminal_id === active)?.alive ?? false}
            autoFocus={focusId === active}
            onError={setError}
          />
        ) : (
          <div className="text-muted-foreground flex h-full flex-col items-center justify-center gap-2 text-xs">
            <SquareTerminal className="size-5" />
            <p>No terminals in this workspace.</p>
            <Button size="xs" variant="outline" onClick={spawn} disabled={!online}>
              New terminal
            </Button>
          </div>
        )}
      </div>
    </div>
  );
}

const MENU_ITEM =
  "hover:bg-muted focus-visible:bg-muted flex w-full items-center gap-2.5 rounded-md px-2.5 py-1.5 text-left text-ui transition-colors disabled:opacity-50";

/**
 * Open: the worktree's first preview in the browser. Its chevron lists them
 * all, asked for anew each time, since servers come and go.
 */
function OpenPreview({
  workspaceId,
  onNone,
  onError,
  onSettings,
}: {
  workspaceId: string;
  onNone(): void;
  onError(msg: string): void;
  onSettings: (() => void) | null;
}) {
  const controller = useController();
  const online = useOnline();
  const root = useRef<HTMLDivElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const [menu, setMenu] = useState(false);
  const [items, setItems] = useState<Preview[] | null>(null);
  const [anchor, setAnchor] = useState<DOMRect | null>(null);
  const presence = usePresence(menu);
  // Only the last ask's answer counts.
  const asked = useRef(0);

  const ask = async () => {
    const n = ++asked.current;
    const { previews } = await controller.client.request("workspace.previews", { workspace_id: workspaceId });
    return n === asked.current ? previews : null;
  };

  useEffect(() => setMenu(false), [workspaceId]);

  useEffect(() => {
    if (!menu) return;
    const onDown = (e: PointerEvent) => {
      const t = e.target as Node;
      if (!root.current?.contains(t) && !list.current?.contains(t)) setMenu(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      setMenu(false);
      root.current?.querySelector<HTMLElement>('[aria-haspopup="menu"]')?.focus();
    };
    window.addEventListener("pointerdown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("pointerdown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [menu]);

  useEffect(() => {
    if (menu && items) focusMenu(list.current);
  }, [menu, items]);

  async function open() {
    try {
      const previews = await ask();
      if (!previews) return;
      if (previews[0]) await openLink(previews[0].url);
      else onNone();
    } catch (e) {
      onError(errorMessage(e));
    }
  }

  function toggle() {
    if (menu) return setMenu(false);
    setAnchor(root.current!.getBoundingClientRect());
    setItems(null);
    setMenu(true);
    ask().then(
      (previews) => previews && setItems(previews),
      (e) => {
        setMenu(false);
        onError(errorMessage(e));
      },
    );
  }

  function pick(run: () => unknown) {
    setMenu(false);
    void run();
  }

  // The dock sits low in the window: the menu opens upward unless there is no room.
  const up = anchor ? anchor.top > window.innerHeight / 2 : true;
  const style: React.CSSProperties | undefined = anchor ? {
    right: window.innerWidth - anchor.right,
    ...(up ? { bottom: window.innerHeight - anchor.top + 4 } : { top: anchor.bottom + 4 }),
    transformOrigin: up ? "bottom right" : "top right",
  } : undefined;

  return (
    <div ref={root} className="flex shrink-0 items-center">
      <Button
        size="xs"
        variant="ghost"
        className="rounded-r-none"
        title="Open the worktree's app in your browser"
        disabled={!online}
        onClick={open}
      >
        <ExternalLink />
        Open
      </Button>
      <Button
        size="xs"
        variant="ghost"
        className="rounded-l-none px-1"
        title="Previews"
        aria-label="Previews"
        aria-haspopup="menu"
        aria-expanded={menu}
        disabled={!online}
        onClick={toggle}
      >
        <ChevronDown />
      </Button>
      {presence.mounted &&
        createPortal(
          <div
            ref={list}
            className="bg-card text-card-foreground ring-border presence-pop shadow-pop fixed z-50 w-72 rounded-xl p-1.5 ring-1 outline-none"
            style={style}
            role="menu"
            tabIndex={-1}
            aria-label="Previews"
            data-state={presence.state}
            aria-hidden={!menu || undefined}
            inert={!menu}
            onKeyDown={moveMenuFocus}
          >
            {items === null ? (
              <p className="text-muted-foreground px-2.5 py-1.5">Looking for servers…</p>
            ) : items.length === 0 ? (
              <p className="text-muted-foreground px-2.5 py-1.5">No servers running in this worktree.</p>
            ) : (
              items.map((p) => (
                <button key={p.url} className={MENU_ITEM} role="menuitem" onClick={() => pick(() => openLink(p.url))}>
                  <ExternalLink className="text-muted-foreground size-4 shrink-0" />
                  <span className="flex min-w-0 flex-col">
                    {p.name && <span className="truncate">{p.name}</span>}
                    <span className={cn("truncate font-mono text-xs", p.name && "text-muted-foreground")}>{p.url}</span>
                  </span>
                </button>
              ))
            )}
            {onSettings && (
              <>
                <div className="bg-border my-1 h-px" role="separator" />
                <button className={MENU_ITEM} role="menuitem" onClick={() => pick(onSettings)}>
                  <Settings className="text-muted-foreground size-4" />
                  Preview URLs…
                </button>
              </>
            )}
          </div>,
          document.body,
        )}
    </div>
  );
}

/**
 * Lines this window keeps of a terminal whose host keeps `bytes` of its
 * output: about one line per 80 bytes, so scrolling back shows what the host
 * would replay, within what xterm draws without strain.
 */
export function scrollbackLines(bytes: number): number {
  return Math.min(50_000, Math.max(1_000, Math.round(bytes / 80)));
}

export function TerminalView({
  controller,
  terminalId,
  alive,
  autoFocus,
  onError,
}: {
  controller: HostController;
  terminalId: string;
  /** An ended terminal stays readable; it takes no input or resizes. */
  alive: boolean;
  autoFocus: boolean;
  onError(msg: string): void;
}) {
  const host = useRef<HTMLDivElement>(null);
  const aliveRef = useRef(alive);
  aliveRef.current = alive;
  const focusRef = useRef(autoFocus);
  focusRef.current = autoFocus;
  const fontFamily = usePref(terminalFonts);
  const fontSize = usePref(() => fonts().terminalSize);
  const shown = useRef<{ term: Terminal; fit(): void } | null>(null);
  const advanced = useHost((s) => hasCapability(s.connection.compat, "settings.advanced"));

  useEffect(() => {
    const el = host.current!;
    const term = new Terminal({
      cursorBlink: true,
      fontSize: fonts().terminalSize,
      fontFamily: terminalFonts(),
      scrollback: scrollbackLines(DEFAULT_SCROLLBACK_BYTES),
      // xterm paints its own canvas: these mirror the `--terminal` token (styles.css), dark in both themes.
      theme: { background: TERMINAL_BG, foreground: TERMINAL_FG },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(el);

    // Listen before attaching so the initial snapshot is not missed.
    const off = controller.onTerminalOutput((id, out) => {
      if (id !== terminalId) return;
      if (out.kind === "reset") {
        term.reset();
        term.write(out.data);
      } else if (out.kind === "append") {
        term.write(out.data);
      } else if (out.kind === "exit") {
        term.write("\r\n\x1b[2m[process exited]\x1b[0m\r\n");
      }
    });
    controller.attachTerminal(terminalId);

    const input = new InputQueue(
      (data) => controller.client.request("terminal.write", { terminal_id: terminalId, data }),
      (e) => onError(errorMessage(e)),
    );
    const onData = term.onData((d) => aliveRef.current && input.push(d));

    let resizeTimer: ReturnType<typeof setTimeout> | undefined;
    const onResize = term.onResize(({ cols, rows }) => {
      clearTimeout(resizeTimer);
      resizeTimer = setTimeout(() => {
        if (!aliveRef.current) return;
        controller.client
          .request("terminal.resize", { terminal_id: terminalId, cols, rows })
          .catch((e) => onError(errorMessage(e)));
      }, 100);
    });
    const refit = () => {
      // A collapsed panel has no size; fitting to it would shrink the PTY.
      if (el.clientWidth === 0 || el.clientHeight === 0) return;
      try {
        fit.fit();
      } catch {
        // not laid out yet
      }
    };
    const observer = new ResizeObserver(refit);
    observer.observe(el);
    shown.current = { term, fit: refit };
    if (focusRef.current) term.focus();

    return () => {
      shown.current = null;
      clearTimeout(resizeTimer);
      observer.disconnect();
      onData.dispose();
      onResize.dispose();
      off();
      term.dispose();
    };
  }, [controller, terminalId, onError]);

  // As much as the host keeps (Settings → Advanced), read when the terminal opens.
  useEffect(() => {
    if (!advanced) return;
    let live = true;
    controller.client.request("settings.get", {}).then(
      (s) => {
        const bytes = s.values["terminal.scrollback"];
        const t = shown.current;
        if (live && t && typeof bytes === "number") t.term.options.scrollback = scrollbackLines(bytes);
      },
      () => {},
    );
    return () => {
      live = false;
    };
  }, [controller, terminalId, advanced]);

  // A font chosen in Settings redraws open terminals, refitted to the new cell size.
  useEffect(() => {
    const t = shown.current;
    if (!t || (t.term.options.fontFamily === fontFamily && t.term.options.fontSize === fontSize)) return;
    t.term.options.fontFamily = fontFamily;
    t.term.options.fontSize = fontSize;
    t.fit();
  }, [fontFamily, fontSize]);

  return <div ref={host} className="animate-fade-in h-full w-full" data-testid="terminal" />;
}
