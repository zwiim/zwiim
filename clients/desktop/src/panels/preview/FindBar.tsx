import { useCallback, useEffect, useRef, useState, type RefObject } from "react";
import { CaseSensitive, ChevronDown, ChevronRight, ChevronUp, Replace, ReplaceAll, Search, WholeWord, X } from "lucide-react";
import type { FindOptions } from "@zwiim/preview";
import { Button } from "@/components/ui/button";
import { isModKey, platform } from "@/lib/platform";
import { cn } from "@/lib/utils";

/** A preview's find state; `current` may run past the matches, so read it through `at`. */
export interface Find {
  open: boolean;
  query: string;
  options: FindOptions;
  current: number;
  /** The replace row is open; it shows only where the preview can replace. */
  replacing: boolean;
  replacement: string;
  input: RefObject<HTMLInputElement | null>;
  show(replace?: boolean): void;
  close(): void;
  setReplacing(replacing: boolean): void;
  setReplacement(replacement: string): void;
  setQuery(query: string): void;
  setOptions(options: FindOptions): void;
  setCurrent(current: number): void;
}

/** The current match's index among `count` matches. */
export const at = (find: Find, count: number) => (count ? Math.min(find.current, count - 1) : 0);

function editable(el: Element | null): boolean {
  return !!el && (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement || (el as HTMLElement).isContentEditable);
}

/** ⌘⌥F on a Mac, Ctrl+H elsewhere: find with the replace row open. */
function isReplaceKey(e: KeyboardEvent): boolean {
  if (platform() === "macos") return e.metaKey && e.altKey && !e.shiftKey && e.code === "KeyF";
  return e.ctrlKey && !e.altKey && !e.shiftKey && e.key.toLowerCase() === "h";
}

/**
 * Find for the preview in `root`. While `active`, ⌘F (Ctrl+F) opens it, and
 * ⌘⌥F (Ctrl+H) opens it to replace, unless a text field outside the preview
 * has focus or another handler took the key.
 */
export function useFind(active: boolean, root: RefObject<HTMLElement | null>): Find {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [options, setOptions] = useState<FindOptions>({});
  const [current, setCurrent] = useState(0);
  const [replacing, setReplacing] = useState(false);
  const [replacement, setReplacement] = useState("");
  const input = useRef<HTMLInputElement>(null);

  const show = useCallback((replace?: boolean) => {
    setOpen(true);
    if (replace) setReplacing(true);
    // The field mounts with the bar; focus it once it has.
    requestAnimationFrame(() => {
      input.current?.focus();
      input.current?.select();
    });
  }, []);
  const close = useCallback(() => {
    setOpen(false);
    setQuery("");
    setCurrent(0);
    setReplacing(false);
  }, []);

  useEffect(() => {
    if (!active) return;
    function onKey(e: KeyboardEvent) {
      const replace = isReplaceKey(e);
      if (e.defaultPrevented) return;
      if (!replace && (!isModKey(e) || e.shiftKey || e.altKey || e.key.toLowerCase() !== "f")) return;
      const focused = document.activeElement;
      if (editable(focused) && !root.current?.contains(focused)) return;
      e.preventDefault();
      show(replace);
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [active, root, show]);

  return {
    open,
    query,
    options,
    current,
    replacing,
    replacement,
    input,
    show,
    close,
    setReplacing,
    setReplacement,
    setQuery: (q) => {
      setQuery(q);
      setCurrent(0);
    },
    setOptions: (o) => {
      setOptions(o);
      setCurrent(0);
    },
    setCurrent,
  };
}

/**
 * The find row under a preview's header. `count` is the number of matches,
 * undefined while they are being counted; `note` replaces the field where the
 * preview cannot be searched. A preview that tracks its own current match
 * (pdf.js) passes it as `current` and moves it in `onStep`. One that can
 * replace passes `onReplace`, called with the current match or all of them.
 */
export function FindBar({
  find,
  count,
  note,
  current: own,
  onStep,
  onReplace,
}: {
  find: Find;
  count: number | undefined;
  note?: string;
  current?: number;
  onStep?: (by: 1 | -1) => void;
  onReplace?: (which: "current" | "all") => void;
}) {
  const n = count ?? 0;
  const current = own ?? at(find, n);
  const step = (by: 1 | -1) => n && (onStep ? onStep(by) : find.setCurrent((current + by + n) % n));
  const toggle = (key: keyof FindOptions, label: string, Icon: typeof Search) => (
    <Button
      size="icon-sm"
      variant={find.options[key] ? "secondary" : "ghost"}
      title={label}
      aria-label={label}
      aria-pressed={!!find.options[key]}
      onClick={() => find.setOptions({ ...find.options, [key]: !find.options[key] })}
    >
      <Icon />
    </Button>
  );
  const escape = (e: React.KeyboardEvent) => {
    e.preventDefault();
    e.stopPropagation();
    find.close();
  };
  const replacing = !!onReplace && find.replacing && !note;
  const field =
    "bg-background ring-border focus-within:ring-ring flex max-w-80 min-w-0 flex-1 items-center gap-2 rounded-md px-2 py-1 ring-1 transition-shadow";
  return (
    <div className="text-muted-foreground shrink-0 border-b px-4 text-xs" role="search">
      <div className="flex h-10 items-center gap-1">
        {onReplace && !note && (
          <Button
            size="icon-sm"
            variant="ghost"
            title={replacing ? "Hide replace" : "Show replace"}
            aria-label="Show replace"
            aria-expanded={replacing}
            onClick={() => find.setReplacing(!find.replacing)}
          >
            <ChevronRight className={cn("transition-transform", replacing && "rotate-90")} />
          </Button>
        )}
        {note ? (
          <p className="flex-1">{note}</p>
        ) : (
          <>
            <label className={field}>
              <Search className="size-3.5 shrink-0" />
              <input
                ref={find.input}
                aria-label="Find"
                placeholder="Find"
                className="text-ui text-foreground min-w-0 flex-1 bg-transparent outline-none"
                value={find.query}
                onChange={(e) => find.setQuery(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    step(e.shiftKey ? -1 : 1);
                  } else if (e.key === "Escape") escape(e);
                }}
              />
            </label>
            <span className="min-w-16 px-1 tabular-nums" aria-live="polite" data-testid="find-count">
              {!find.query || count === undefined ? "" : n ? `${current + 1} of ${n}` : "No results"}
            </span>
            {toggle("matchCase", "Match case", CaseSensitive)}
            {toggle("wholeWord", "Whole word", WholeWord)}
            <Button size="icon-sm" variant="ghost" title="Previous match" aria-label="Previous match" disabled={!n} onClick={() => step(-1)}>
              <ChevronUp />
            </Button>
            <Button size="icon-sm" variant="ghost" title="Next match" aria-label="Next match" disabled={!n} onClick={() => step(1)}>
              <ChevronDown />
            </Button>
          </>
        )}
        <Button size="icon-sm" variant="ghost" className="ml-auto" title="Close find" aria-label="Close find" onClick={find.close}>
          <X />
        </Button>
      </div>
      {replacing && (
        <div className="flex h-10 items-center gap-1 pl-8">
          <label className={field}>
            <Replace className="size-3.5 shrink-0" />
            <input
              aria-label="Replace with"
              placeholder="Replace"
              className="text-ui text-foreground min-w-0 flex-1 bg-transparent outline-none"
              value={find.replacement}
              onChange={(e) => find.setReplacement(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && n) {
                  e.preventDefault();
                  onReplace("current");
                } else if (e.key === "Escape") escape(e);
              }}
            />
          </label>
          <Button size="xs" variant="ghost" disabled={!n} onClick={() => onReplace("current")}>
            <Replace /> Replace
          </Button>
          <Button size="xs" variant="ghost" disabled={!n} onClick={() => onReplace("all")}>
            <ReplaceAll /> Replace all
          </Button>
        </div>
      )}
    </div>
  );
}
