import { useEffect, useRef, useState } from "react";
import { SquareTerminal } from "lucide-react";
import type { SessionCommand } from "@zwiim/sdk";
import { cn } from "@/lib/utils";

/** `/query` being typed just before the caret, at the start or after a space. */
export function slashQuery(text: string, caret: number): { start: number; query: string } | null {
  const m = /(^|\s)\/(\S*)$/.exec(text.slice(0, caret));
  return m ? { start: m.index + m[1]!.length, query: m[2]! } : null;
}

/** Commands matching `query`: names starting with it first, then containing it. */
export function matchCommands(commands: readonly SessionCommand[], query: string): SessionCommand[] {
  const q = query.toLowerCase();
  const starts = commands.filter((c) => c.name.toLowerCase().startsWith(q));
  const contains = commands.filter((c) => !c.name.toLowerCase().startsWith(q) && c.name.toLowerCase().includes(q));
  return [...starts, ...contains];
}

/**
 * Commands the agents list whose real screen is their CLI's: over ACP they
 * only reply in text, so they run in the embedded terminal instead.
 */
export const CLI_SCREEN_COMMANDS: ReadonlySet<string> = new Set(["mcp", "model", "config", "status"]);

/**
 * The command to offer in the agent's CLI: a `/name` opening the message
 * that no command the agent lists starts with, so only its CLI runs it
 * (Claude Code's `/login`, Codex's `/model`). Before the agent lists its
 * commands, any.
 */
export function terminalCommand(
  found: { start: number; query: string } | null,
  commands: readonly SessionCommand[] | null | undefined,
): string | null {
  if (!found || found.start !== 0 || !/^[a-z][\w:.-]*$/i.test(found.query)) return null;
  if (commands?.some((c) => c.name.startsWith(found.query))) return null;
  return `/${found.query}`;
}

/** Running a slash command in the agent's CLI, in an embedded terminal. */
export interface TerminalCommands {
  /** The agent's name, as the menu says whose CLI runs the command. */
  agentName: string;
  run(command: string): void;
}

/**
 * The agent's slash commands as a menu over a message box. Picking one
 * writes `/name ` into the text; the agent runs it when the message is sent.
 * One it does not list, or one whose screen is its CLI's, runs in the CLI
 * instead, in an embedded terminal.
 */
export function useSlashMenu({
  text,
  setText,
  area,
  commands,
  terminal,
  below = false,
}: {
  text: string;
  setText(text: string): void;
  area: React.RefObject<HTMLTextAreaElement | null>;
  commands: readonly SessionCommand[] | null | undefined;
  /** Where a command the agent does not run itself can open. */
  terminal?: TerminalCommands | null;
  /** Open under the box instead of over it. */
  below?: boolean;
}) {
  const [caret, setCaret] = useState(0);
  const [active, setActive] = useState(0);
  /** The `/query` start the user dismissed with Escape. */
  const [dismissed, setDismissed] = useState<number | null>(null);
  const list = useRef<HTMLUListElement>(null);

  const found = slashQuery(text, caret);
  const matches = found && commands ? matchCommands(commands, found.query) : [];
  const inTerminal = terminal ? terminalCommand(found, commands) : null;
  const inCli = (c: SessionCommand) => !!terminal && found?.start === 0 && CLI_SCREEN_COMMANDS.has(c.name);
  const count = matches.length + (inTerminal ? 1 : 0);
  const open = found !== null && found.start !== dismissed && count > 0;
  const query = found?.query;

  useEffect(() => setActive(0), [query]);
  useEffect(() => {
    list.current?.querySelector(`[data-index="${active}"]`)?.scrollIntoView?.({ block: "nearest" });
  }, [active]);

  function pick(command: SessionCommand) {
    if (!found) return;
    if (inCli(command)) return runInTerminal(`/${command.name}`);
    const insert = `/${command.name} `;
    const next = text.slice(0, found.start) + insert + text.slice(caret);
    const at = found.start + insert.length;
    setText(next);
    setCaret(at);
    requestAnimationFrame(() => {
      area.current?.focus();
      area.current?.setSelectionRange(at, at);
    });
  }

  function runInTerminal(command: string) {
    if (!found || !terminal) return;
    // The command leaves the message: it runs in the terminal, not here.
    const next = text.slice(0, found.start) + text.slice(caret).trimStart();
    setText(next);
    setCaret(found.start);
    terminal.run(command);
  }

  function choose(i: number) {
    const command = matches[i];
    if (command) pick(command);
    else if (inTerminal) runInTerminal(inTerminal);
  }

  /** Handles the key when the menu is open; true if it did. */
  function onKeyDown(e: React.KeyboardEvent): boolean {
    if (!open || e.nativeEvent.isComposing) return false;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      const step = e.key === "ArrowDown" ? 1 : -1;
      setActive((i) => (i + step + count) % count);
    } else if ((e.key === "Enter" && !e.shiftKey) || e.key === "Tab") {
      choose(Math.min(active, count - 1));
    } else if (e.key === "Escape") {
      setDismissed(found!.start);
    } else {
      return false;
    }
    e.preventDefault();
    e.stopPropagation();
    return true;
  }

  /** Keep the caret known: the menu follows what is typed just before it. */
  const track = (e: React.SyntheticEvent<HTMLTextAreaElement>) => {
    setCaret(e.currentTarget.selectionStart);
    if (dismissed !== null && slashQuery(e.currentTarget.value, e.currentTarget.selectionStart)?.start !== dismissed) {
      setDismissed(null);
    }
  };

  const menu = open && (
    <ul
      ref={list}
      role="listbox"
      aria-label="Commands"
      className={cn(
        "bg-card text-card-foreground absolute left-0 z-30 max-h-64 w-full overflow-y-auto rounded-xl border p-1 shadow-xl",
        below ? "top-full mt-1" : "bottom-full mb-2",
      )}
    >
      {matches.map((c, i) => (
        <li
          key={c.name}
          role="option"
          aria-selected={i === active}
          data-index={i}
          className={cn(
            "flex cursor-default items-baseline gap-2 rounded-md px-2.5 py-1.5 text-sm",
            i === active && "bg-muted",
          )}
          onMouseEnter={() => setActive(i)}
          onMouseDown={(e) => {
            e.preventDefault();
            pick(c);
          }}
        >
          <span className="shrink-0 font-mono text-[13px]">/{c.name}</span>
          {c.hint && <span className="text-muted-foreground shrink-0 font-mono text-xs">{c.hint}</span>}
          <span className="text-muted-foreground min-w-0 truncate text-xs">{c.description}</span>
          {inCli(c) && (
            <span className="text-muted-foreground ml-auto flex shrink-0 items-center gap-1 self-center text-xs">
              <SquareTerminal className="size-3.5" />
              Embedded terminal
            </span>
          )}
        </li>
      ))}
      {inTerminal && terminal && (
        <li
          role="option"
          aria-selected={active === matches.length}
          data-index={matches.length}
          className={cn(
            "flex cursor-default items-center gap-2 rounded-md px-2.5 py-1.5 text-sm",
            active === matches.length && "bg-muted",
            matches.length > 0 && "mt-1",
          )}
          onMouseEnter={() => setActive(matches.length)}
          onMouseDown={(e) => {
            e.preventDefault();
            runInTerminal(inTerminal);
          }}
        >
          <SquareTerminal className="text-muted-foreground size-3.5 shrink-0" />
          <span className="shrink-0">
            Run <span className="font-mono text-[13px]">{inTerminal}</span> in the embedded terminal
          </span>
          <span className="text-muted-foreground min-w-0 truncate text-xs">{terminal.agentName}</span>
        </li>
      )}
    </ul>
  );

  return { menu, onKeyDown, track, open };
}
