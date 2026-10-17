import { createContext, useCallback, useContext, useMemo, useState } from "react";
import { Loader2, SquareTerminal } from "lucide-react";
import { Button } from "@/components/ui/button";
import { errorMessage, useController, useHost } from "@/lib/host-context";
import { TerminalView } from "./TerminalPanel";

/** A slash command running in the agent's CLI, in a terminal of the workspace. */
export interface AgentTerminalRun {
  terminalId: string;
  command: string;
}

export interface AgentTerminals {
  /** Each workspace's command, shown over its composer. */
  runs: Readonly<Record<string, AgentTerminalRun>>;
  /** Every terminal opened this way; the terminal dock leaves them out. */
  ids: ReadonlySet<string>;
  open(workspaceId: string, run: AgentTerminalRun): void;
  close(workspaceId: string): void;
  /** Gives a terminal back to the dock: one that could not be ended. */
  release(terminalId: string): void;
}

export const AgentTerminalsContext = createContext<AgentTerminals | null>(null);

export function useAgentTerminalsState(): AgentTerminals {
  const [runs, setRuns] = useState<Record<string, AgentTerminalRun>>({});
  const [ids, setIds] = useState<ReadonlySet<string>>(() => new Set());
  const open = useCallback((workspaceId: string, run: AgentTerminalRun) => {
    setIds((s) => new Set(s).add(run.terminalId));
    setRuns((r) => ({ ...r, [workspaceId]: run }));
  }, []);
  const close = useCallback((workspaceId: string) => {
    setRuns(({ [workspaceId]: _closed, ...rest }) => rest);
  }, []);
  const release = useCallback((terminalId: string) => {
    setIds((s) => {
      const next = new Set(s);
      next.delete(terminalId);
      return next;
    });
  }, []);
  return useMemo(() => ({ runs, ids, open, close, release }), [runs, ids, open, close, release]);
}

/**
 * The workspace's slash command running in the agent's CLI, over the
 * composer: the user finishes it there, then Done ends the CLI.
 */
export function AgentTerminalCard({ workspaceId }: { workspaceId: string }) {
  const controller = useController();
  const terminals = useContext(AgentTerminalsContext);
  const run = terminals?.runs[workspaceId] ?? null;
  const alive = useHost((s) => (run ? (s.terminals[run.terminalId]?.alive ?? false) : false));
  const [error, setError] = useState<string | null>(null);
  const [ending, setEnding] = useState(false);
  if (!terminals || !run) return null;

  async function done() {
    setError(null);
    setEnding(true);
    try {
      if (alive) await controller.client.request("terminal.kill", { terminal_id: run!.terminalId });
      else controller.detachTerminal(run!.terminalId);
      terminals!.close(workspaceId);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setEnding(false);
    }
  }

  return (
    <section
      className="bg-card overflow-hidden rounded-2xl shadow-lg ring-1 ring-border"
      aria-label={`Running ${run.command}`}
      data-testid="agent-terminal"
    >
      <div className="flex h-10 items-center gap-2 border-b pr-2 pl-4 text-sm">
        <SquareTerminal className="text-muted-foreground size-4 shrink-0" />
        <p className="text-muted-foreground min-w-0 flex-1 truncate">
          {alive ? (
            <>
              Complete <span className="text-foreground font-mono text-ui">{run.command}</span> below, then click
              Done
            </>
          ) : (
            <>
              <span className="font-mono text-ui">{run.command}</span> has finished
            </>
          )}
        </p>
        {error && (
          <span className="text-destructive truncate text-xs" role="alert">
            {error}
          </span>
        )}
        <Button size="xs" variant="outline" disabled={ending} onClick={done}>
          {ending && <Loader2 className="animate-spin" />}
          Done
        </Button>
      </div>
      <div className="h-72 bg-terminal p-1">
        <TerminalView
          controller={controller}
          terminalId={run.terminalId}
          alive={alive}
          autoFocus
          onError={setError}
        />
      </div>
    </section>
  );
}
