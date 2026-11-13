// What the Host's engine does not paint into the stream, drawn over the page
// (design 9): the page's context menu, a <select>'s list, a date or time
// input's picker, and the find bar.

import { useEffect, useRef, useState } from "react";
import { ChevronDown, ChevronUp, X } from "lucide-react";
import type { BrowserMenu, BrowserUiAnswer, BrowserUiRequest } from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import { moveMenuFocus } from "@/panels/ControlCenter";
import { cn } from "@/lib/utils";

export const POP = "bg-card text-card-foreground ring-border presence-pop shadow-pop absolute z-20 rounded-xl p-1.5 ring-1 outline-none";
const ITEM =
  "hover:bg-muted focus-visible:bg-muted flex w-full items-center gap-2.5 rounded-md px-2.5 py-1.5 text-left text-ui transition-colors outline-none disabled:opacity-50";

type Place = (p: { x: number; y: number }) => { x: number; y: number } | null;

/** Closes on a press outside `ref`. */
export function useDismiss(ref: React.RefObject<HTMLElement | null>, onClose: () => void) {
  useEffect(() => {
    const down = (e: PointerEvent) => {
      if (!ref.current?.contains(e.target as Node)) onClose();
    };
    window.addEventListener("pointerdown", down, true);
    return () => window.removeEventListener("pointerdown", down, true);
  }, [ref, onClose]);
}

/** A popover's position at `at` in the view, kept inside it. */
function within(at: { x: number; y: number }, view: HTMLElement | null, size: { width: number; height: number }) {
  const w = view?.clientWidth ?? Infinity;
  const h = view?.clientHeight ?? Infinity;
  return {
    left: Math.max(4, Math.min(at.x, w - size.width - 4)),
    top: Math.max(4, Math.min(at.y, h - size.height - 4)),
  };
}

export type MenuAction =
  | { kind: "step"; action: "back" | "forward" | "reload" }
  | { kind: "open"; url: string }
  | { kind: "copy"; text: string }
  | { kind: "paste" };

/** The page's right-click menu, where the person clicked. */
export function PageMenu({
  menu,
  place,
  view,
  onAction,
  onClose,
}: {
  menu: BrowserMenu;
  place: Place;
  view: HTMLElement | null;
  onAction(a: MenuAction): void;
  onClose(): void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useDismiss(ref, onClose);
  useEffect(() => ref.current?.focus(), []);
  const at = place(menu);
  if (!at) return null;
  const items: { label: string; action: MenuAction }[] = [];
  if (menu.link_url) {
    items.push({ label: "Open link in new tab", action: { kind: "open", url: menu.link_url } });
    items.push({ label: "Copy link address", action: { kind: "copy", text: menu.link_url } });
  }
  if (menu.image_url) {
    items.push({ label: "Open image in new tab", action: { kind: "open", url: menu.image_url } });
    items.push({ label: "Copy image address", action: { kind: "copy", text: menu.image_url } });
  }
  if (menu.selection) items.push({ label: "Copy", action: { kind: "copy", text: menu.selection } });
  if (menu.editable) items.push({ label: "Paste", action: { kind: "paste" } });
  const page: { label: string; action: MenuAction }[] = [
    { label: "Back", action: { kind: "step", action: "back" } },
    { label: "Forward", action: { kind: "step", action: "forward" } },
    { label: "Reload", action: { kind: "step", action: "reload" } },
  ];
  const row = (i: { label: string; action: MenuAction }) => (
    <button
      key={i.label}
      className={ITEM}
      role="menuitem"
      onClick={() => {
        onClose();
        onAction(i.action);
      }}
    >
      {i.label}
    </button>
  );
  return (
    <div
      ref={ref}
      className={cn(POP, "w-52")}
      style={within(at, view, { width: 208, height: 36 * (items.length + 3) + 16 })}
      role="menu"
      aria-label="Page menu"
      tabIndex={-1}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === "Escape") onClose();
        else moveMenuFocus(e);
      }}
    >
      {items.map(row)}
      {items.length > 0 && <div className="bg-border my-1 h-px" role="separator" />}
      {page.map(row)}
    </div>
  );
}

type SelectRequest = Extract<BrowserUiRequest, { kind: "select" }>;
type DateRequest = Extract<BrowserUiRequest, { kind: "date" }>;
export type PickRequest = SelectRequest | DateRequest;

export function isPick(r: BrowserUiRequest): r is PickRequest {
  return r.kind === "select" || r.kind === "date";
}

/** A `<select>`'s list, under the select: arrows move, Enter picks, Escape cancels. */
export function SelectPicker({
  request,
  place,
  view,
  onAnswer,
}: {
  request: SelectRequest;
  place: Place;
  view: HTMLElement | null;
  onAnswer(a: BrowserUiAnswer): void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const cancel = () => onAnswer({ kind: "select", accept: false });
  useDismiss(ref, cancel);
  const enabled = request.options.filter((o) => !o.disabled);
  const [active, setActive] = useState(() => (request.options.find((o) => o.selected && !o.disabled) ?? enabled[0])?.index);
  useEffect(() => ref.current?.focus(), []);
  useEffect(() => {
    ref.current?.querySelector(`[data-index="${active}"]`)?.scrollIntoView?.({ block: "nearest" });
  }, [active]);
  const top = place({ x: request.x, y: request.y + request.height });
  const end = place({ x: request.x + request.width, y: request.y });
  if (!top || !end) return null;
  const width = Math.max(160, end.x - top.x);
  const step = (by: number) => {
    const i = enabled.findIndex((o) => o.index === active);
    const next = enabled[Math.max(0, Math.min(enabled.length - 1, i + by))];
    if (next) setActive(next.index);
  };
  let group: string | undefined;
  return (
    <div
      ref={ref}
      className={cn(POP, "max-h-72 overflow-y-auto")}
      style={{ ...within(top, view, { width, height: 288 }), width }}
      role="listbox"
      aria-label="Choose an option"
      aria-activedescendant={active === undefined ? undefined : `pick-${active}`}
      tabIndex={-1}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === "Escape") cancel();
        else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
          e.preventDefault();
          step(e.key === "ArrowDown" ? 1 : -1);
        } else if ((e.key === "Enter" || e.key === " ") && active !== undefined) {
          e.preventDefault();
          onAnswer({ kind: "select", accept: true, index: active });
        }
      }}
    >
      {request.options.map((o) => {
        const header = o.group !== group && o.group ? o.group : null;
        group = o.group;
        return (
          <div key={o.index}>
            {header && <p className="text-muted-foreground px-2.5 pt-1.5 pb-0.5 text-xs">{header}</p>}
            <button
              id={`pick-${o.index}`}
              data-index={o.index}
              role="option"
              aria-selected={o.selected}
              disabled={o.disabled}
              className={cn(ITEM, o.group && "pl-5", o.index === active && "bg-muted", o.selected && "font-medium")}
              onPointerEnter={() => !o.disabled && setActive(o.index)}
              onClick={() => onAnswer({ kind: "select", accept: true, index: o.index })}
            >
              <span className="truncate">{o.label || " "}</span>
            </button>
          </div>
        );
      })}
    </div>
  );
}

/** A date or time input's picker, under the input. */
export function DatePicker({
  request,
  place,
  view,
  onAnswer,
}: {
  request: DateRequest;
  place: Place;
  view: HTMLElement | null;
  onAnswer(a: BrowserUiAnswer): void;
}) {
  const ref = useRef<HTMLFormElement>(null);
  const [value, setValue] = useState(request.value);
  const cancel = () => onAnswer({ kind: "date", accept: false });
  useDismiss(ref, cancel);
  useEffect(() => ref.current?.querySelector("input")?.focus(), []);
  const at = place({ x: request.x, y: request.y + request.height });
  if (!at) return null;
  return (
    <form
      ref={ref}
      className={cn(POP, "flex w-64 flex-col gap-2 p-3")}
      style={within(at, view, { width: 256, height: 100 })}
      aria-label="Choose a date"
      onSubmit={(e) => {
        e.preventDefault();
        onAnswer({ kind: "date", accept: true, value });
      }}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === "Escape") cancel();
      }}
    >
      <input
        type={request.input_type}
        aria-label="Value"
        className="border-border bg-input/30 text-ui h-8 rounded-md border px-2 outline-none focus-visible:ring-2"
        value={value}
        min={request.min}
        max={request.max}
        onChange={(e) => setValue(e.target.value)}
      />
      <div className="flex justify-end gap-2">
        <Button type="button" size="xs" variant="ghost" onClick={() => onAnswer({ kind: "date", accept: true, value: "" })}>
          Clear
        </Button>
        <Button type="button" size="xs" variant="outline" onClick={cancel}>
          Cancel
        </Button>
        <Button type="submit" size="xs">
          Done
        </Button>
      </div>
    </form>
  );
}

/** Find in page: Enter goes to the next match, Shift+Enter the one before, Escape closes. */
export function FindBar({
  find,
  onClose,
}: {
  find(query: string, backwards: boolean): Promise<{ matches: number; current: number } | null>;
  onClose(): void;
}) {
  const [query, setQuery] = useState("");
  const [found, setFound] = useState<{ matches: number; current: number } | null>(null);
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => input.current?.focus(), []);
  // A new query searches from the start once typing pauses.
  useEffect(() => {
    if (!query) {
      setFound(null);
      return;
    }
    const t = setTimeout(() => void find(query, false).then(setFound), 150);
    return () => clearTimeout(t);
  }, [query, find]);
  const go = (backwards: boolean) => {
    if (query) void find(query, backwards).then(setFound);
  };
  return (
    <div
      className="bg-card ring-border shadow-pop presence-pop absolute top-2 right-2 z-20 flex items-center gap-1 rounded-lg p-1 ring-1"
      role="search"
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === "Escape") {
          e.preventDefault();
          onClose();
        }
      }}
    >
      <input
        ref={input}
        aria-label="Find in page"
        placeholder="Find in page"
        className="text-ui h-7 w-44 bg-transparent px-2 outline-none"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            go(e.shiftKey);
          }
        }}
      />
      <span className="text-muted-foreground w-14 text-right text-xs tabular-nums" aria-live="polite">
        {found ? (found.matches ? `${found.current}/${found.matches}` : "No matches") : ""}
      </span>
      <Button size="icon-sm" variant="ghost" title="Previous match" disabled={!found?.matches} onClick={() => go(true)}>
        <ChevronUp />
      </Button>
      <Button size="icon-sm" variant="ghost" title="Next match" disabled={!found?.matches} onClick={() => go(false)}>
        <ChevronDown />
      </Button>
      <Button size="icon-sm" variant="ghost" title="Close" onClick={onClose}>
        <X />
      </Button>
    </div>
  );
}
