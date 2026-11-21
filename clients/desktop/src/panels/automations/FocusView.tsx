import { useEffect, useRef, useState } from "react";
import { ArrowLeft, ArrowRight, ChevronLeft } from "lucide-react";
import { Button } from "@/components/ui/button";
import type { PlaybookRow } from "./data";
import { FocusPanel } from "./FocusPanel";
import { hostOf, titleOf } from "./ItemTile";
import { LiveScreen } from "./LiveScreen";

/** The focus view streams the page at the narrow width item tabs run at. */
const PAGE = { width: 430, height: 932 };
const PAINT_MS = 100;

/**
 * One item large: its page live in a phone frame, the run's other items a
 * key away, and what it waits on the person for beside it.
 */
export function FocusView({
  row,
  itemId,
  onMove,
  onClose,
}: {
  row: PlaybookRow;
  itemId: string;
  onMove(itemId: string): void;
  onClose(): void;
}) {
  const index = row.items.findIndex((i) => i.item_id === itemId);
  // A new snapshot empties the items until they load again: keep showing the last one meanwhile.
  const last = useRef(index >= 0 ? row.items[index]! : null);
  if (index >= 0) last.current = row.items[index]!;
  const item = index >= 0 ? row.items[index]! : row.itemsLoaded ? null : last.current;
  const prev = index > 0 ? row.items[index - 1]! : null;
  const next = index >= 0 && index < row.items.length - 1 ? row.items[index + 1]! : null;
  const [refused, setRefused] = useState<string | null>(null);

  // The item left the run's list (a new listing dropped it): nothing to show.
  useEffect(() => {
    if (!item) onClose();
  }, [item, onClose]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // Keys typed into a field stay there, and a dialog or menu over the view takes its own keys.
      if (e.defaultPrevented) return;
      if (e.target instanceof Element && e.target.closest("input, textarea, select, [contenteditable='true']")) return;
      if (document.querySelector('[role="dialog"], [role="menu"], [role="listbox"]')) return;
      if (e.key === "Escape") onClose();
      else if (e.key === "ArrowLeft" && prev) onMove(prev.item_id);
      else if (e.key === "ArrowRight" && next) onMove(next.item_id);
      else return;
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [prev, next, onMove, onClose]);

  if (!item) return null;
  const title = titleOf(item);
  const live = !!item.tab_id && refused !== item.tab_id;

  return (
    <div className="flex min-h-0 flex-1 flex-col" data-testid="focus-view">
      <header className="flex items-center gap-2 border-b px-4 py-2">
        <Button variant="ghost" size="xs" onClick={onClose}>
          <ChevronLeft />
          Automations
        </Button>
        <span className="flex-1" />
        <span className="text-muted-foreground font-mono text-xs tabular-nums">
          {index + 1} of {row.items.length}
        </span>
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label="Previous item"
          disabled={!prev}
          onClick={() => prev && onMove(prev.item_id)}
        >
          <ArrowLeft />
        </Button>
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label="Next item"
          disabled={!next}
          onClick={() => next && onMove(next.item_id)}
        >
          <ArrowRight />
        </Button>
      </header>
      <div className="flex min-h-0 flex-1">
        <div className="flex min-w-0 flex-1 items-center justify-center p-6">
          <div className="bg-card flex aspect-[430/932] h-full max-h-[932px] flex-col overflow-hidden rounded-[14px] border">
            <div className="text-muted-foreground border-border truncate border-b px-3 py-1.5 font-mono text-xs">
              {item.url ?? item.site}
            </div>
            <div className="relative min-h-0 flex-1">
              {live ? (
                <LiveScreen
                  key={item.tab_id}
                  tabId={item.tab_id!}
                  title={title}
                  viewport={PAGE}
                  paintMs={PAINT_MS}
                  onRefused={() => setRefused(item.tab_id!)}
                />
              ) : (
                <div className="text-muted-foreground flex h-full items-center justify-center px-6 text-center text-sm">
                  {item.site ?? hostOf(item.url)}
                </div>
              )}
            </div>
          </div>
        </div>
        <FocusPanel key={item.item_id} playbookName={row.playbook.name} item={item} />
      </div>
    </div>
  );
}
