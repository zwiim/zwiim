import { useState } from "react";
import type { RunItemView } from "@zwiim/sdk";
import { cn } from "@/lib/utils";
import { LiveScreen } from "./LiveScreen";
import { itemStatus } from "./status";
import { StatusDot } from "./StatusDot";

/** The tile's screen, in CSS pixels: a phone held upright (430:932). */
const WIDTH = 156;
const HEIGHT = 338;
/** A tile paints at most this often; the newest frame wins. */
const PAINT_MS = 250;

export function hostOf(url: string | null | undefined): string | null {
  if (!url) return null;
  try {
    return new URL(url).host;
  } catch {
    return null;
  }
}

/** The item's title, else the last part of its page's path. */
export function titleOf(item: RunItemView): string {
  if (item.title) return item.title;
  try {
    return new URL(item.url ?? "").pathname.split("/").filter(Boolean).at(-1) ?? item.item_id;
  } catch {
    return item.item_id;
  }
}

/**
 * One item of a run as a small phone: its live page while it has a tab, a
 * card with its site otherwise, and what waits on the person over the page.
 */
export function ItemTile({ item, onOpen }: { item: RunItemView; onOpen?(): void }) {
  const status = itemStatus(item);
  const title = titleOf(item);
  const site = hostOf(item.url) ?? item.site ?? "";
  const [refused, setRefused] = useState<string | null>(null);
  const live = !!item.tab_id && refused !== item.tab_id;

  return (
    <button
      type="button"
      className="focus-visible:ring-ring flex w-[156px] shrink-0 flex-col gap-1.5 rounded-[14px] text-left outline-none focus-visible:ring-2"
      onClick={onOpen}
    >
      <div
        data-testid="tile-frame"
        className={cn(
          "bg-card relative flex h-[338px] w-[156px] flex-col overflow-hidden rounded-[14px] border transition-opacity duration-120",
          status.dot === "need" && "ring-brand ring-2",
          status.dot === "off" && "opacity-50",
        )}
      >
        <div className="text-muted-foreground border-border truncate border-b px-2 py-1 font-mono text-2xs">{site}</div>
        <div className="relative min-h-0 flex-1">
          {live ? (
            <LiveScreen
              tabId={item.tab_id!}
              title={title}
              viewport={{ width: WIDTH, height: HEIGHT }}
              paintMs={PAINT_MS}
              onRefused={() => setRefused(item.tab_id!)}
            />
          ) : (
            <div
              className="text-muted-foreground flex h-full items-center justify-center px-3 text-center text-xs"
              data-testid="tile-face"
            >
              {item.site ?? site}
            </div>
          )}
        </div>
        {status.overlay && (
          <div className="bg-brand text-brand-foreground absolute inset-x-0 bottom-0 px-2 py-1.5 text-xs leading-snug">
            {status.overlay}
          </div>
        )}
      </div>
      <span className="text-ui truncate" title={title}>
        {title}
      </span>
      <span className="text-muted-foreground flex items-center gap-1 text-xs">
        <StatusDot dot={status.dot} />
        {status.label}
      </span>
    </button>
  );
}
