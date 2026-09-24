import { useEffect, useRef, useState } from "react";
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
 * The agent's slash commands as a menu over a message box. Picking one
 * writes `/name ` into the text; the agent runs it when the message is sent.
 */
export function useSlashMenu({
  text,
  setText,
  area,
  commands,
  below = false,
}: {
  text: string;
  setText(text: string): void;
  area: React.RefObject<HTMLTextAreaElement | null>;
  commands: readonly SessionCommand[] | null | undefined;
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
  const open = found !== null && found.start !== dismissed && matches.length > 0;
  const query = found?.query;

  useEffect(() => setActive(0), [query]);
  useEffect(() => {
    list.current?.querySelector(`[data-index="${active}"]`)?.scrollIntoView?.({ block: "nearest" });
  }, [active]);

  function pick(command: SessionCommand) {
    if (!found) return;
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

  /** Handles the key when the menu is open; true if it did. */
  function onKeyDown(e: React.KeyboardEvent): boolean {
    if (!open || e.nativeEvent.isComposing) return false;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      const step = e.key === "ArrowDown" ? 1 : -1;
      setActive((i) => (i + step + matches.length) % matches.length);
    } else if ((e.key === "Enter" && !e.shiftKey) || e.key === "Tab") {
      pick(matches[Math.min(active, matches.length - 1)]!);
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
        </li>
      ))}
    </ul>
  );

  return { menu, onKeyDown, track, open };
}
