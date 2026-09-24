import { useEffect, useMemo, useRef, useState } from "react";
import { Plus, X } from "lucide-react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import type { HostController } from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import { errorMessage, useController, useHost, useOnline } from "@/lib/host-context";
import { cn } from "@/lib/utils";

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

export function TerminalPanel({ workspaceId }: { workspaceId: string }) {
  const controller = useController();
  const online = useOnline();
  const terminals = useHost((s) => s.terminals);
  const [active, setActive] = useState<string | null>(null);
  const [confirmKill, setConfirmKill] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Only this workspace's terminals; others belong to their own workspaces.
  const list = useMemo(
    () => Object.values(terminals).filter((t) => t.workspace_id === workspaceId),
    [terminals, workspaceId],
  );

  useEffect(() => {
    if (!active || !list.some((t) => t.terminal_id === active)) setActive(list[0]?.terminal_id ?? null);
    if (confirmKill && !list.some((t) => t.terminal_id === confirmKill && t.alive)) setConfirmKill(null);
  }, [active, confirmKill, list]);

  async function spawn() {
    setError(null);
    try {
      setActive(await controller.spawnTerminal(workspaceId));
    } catch (e) {
      setError(errorMessage(e));
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
      <div className="flex items-center gap-1 overflow-x-auto border-b px-2 py-1" role="tablist" aria-label="Terminals">
        {list.map((t, i) => (
          <div
            key={t.terminal_id}
            className={cn(
              "flex shrink-0 items-center gap-1 rounded-md px-2 py-0.5 text-xs",
              active === t.terminal_id ? "bg-muted font-medium" : "hover:bg-muted/60",
              !t.alive && "text-muted-foreground",
            )}
          >
            <button
              role="tab"
              aria-selected={active === t.terminal_id}
              title={t.alive ? undefined : "This terminal has exited"}
              onClick={() => setActive(t.terminal_id)}
            >
              {t.title || `Terminal ${i + 1}`}
              {!t.alive && " (exited)"}
            </button>
            {t.alive ? (
              <button
                title="Kill terminal"
                aria-label={`Kill ${t.title || `Terminal ${i + 1}`}`}
                disabled={!online}
                onClick={() => setConfirmKill(t.terminal_id)}
              >
                <X className="size-3" />
              </button>
            ) : (
              <button title="Close" aria-label="Close" onClick={() => controller.detachTerminal(t.terminal_id)}>
                <X className="size-3" />
              </button>
            )}
          </div>
        ))}
        <Button size="icon-sm" variant="ghost" title="New terminal" onClick={spawn} disabled={!online}>
          <Plus />
        </Button>
        {error && <span className="text-destructive ml-2 text-xs">{error}</span>}
      </div>
      {confirmKill && (
        <div
          className="bg-destructive/5 flex items-center gap-2 border-b px-3 py-1.5 text-xs"
          data-testid="confirm-kill"
        >
          <span className="flex-1">
            Kill {labelOf(confirmKill)}? Anything running in it stops.
          </span>
          <Button size="xs" variant="destructive" disabled={!online} onClick={() => kill(confirmKill)}>
            Kill
          </Button>
          <Button size="xs" variant="ghost" onClick={() => setConfirmKill(null)}>
            Keep
          </Button>
        </div>
      )}
      <div className="min-h-0 flex-1 bg-[#0c0c0c] p-1">
        {active ? (
          <TerminalView
            key={active}
            controller={controller}
            terminalId={active}
            alive={list.find((t) => t.terminal_id === active)?.alive ?? false}
            onError={setError}
          />
        ) : (
          <p className="text-muted-foreground p-4 text-xs">No terminals in this workspace. Open one with +.</p>
        )}
      </div>
    </div>
  );
}

function TerminalView({
  controller,
  terminalId,
  alive,
  onError,
}: {
  controller: HostController;
  terminalId: string;
  /** An ended terminal stays readable; it takes no input or resizes. */
  alive: boolean;
  onError(msg: string): void;
}) {
  const host = useRef<HTMLDivElement>(null);
  const aliveRef = useRef(alive);
  aliveRef.current = alive;

  useEffect(() => {
    const el = host.current!;
    const term = new Terminal({
      cursorBlink: true,
      fontSize: 13,
      fontFamily: '"JetBrains Mono", Menlo, Monaco, "Courier New", monospace',
      scrollback: 5000,
      theme: { background: "#0c0c0c" },
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
    const observer = new ResizeObserver(() => {
      try {
        fit.fit();
      } catch {
        // not laid out yet
      }
    });
    observer.observe(el);
    term.focus();

    return () => {
      clearTimeout(resizeTimer);
      observer.disconnect();
      onData.dispose();
      onResize.dispose();
      off();
      term.dispose();
    };
  }, [controller, terminalId, onError]);

  return <div ref={host} className="h-full w-full" data-testid="terminal" />;
}
