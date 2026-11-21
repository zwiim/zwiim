import { useEffect, useRef, useState } from "react";
import type { BrowserScreenFrame, BrowserStream, RunItemView } from "@zwiim/sdk";
import { useController } from "@/lib/host-context";
import { cn } from "@/lib/utils";
import { itemStatus } from "./status";
import { StatusDot } from "./StatusDot";

/** The tile's screen, in CSS pixels: a phone held upright (430:932). */
const WIDTH = 156;
const HEIGHT = 338;
/** A tile paints at most this often; the newest frame wins. */
const PAINT_MS = 250;
/** A stream that lapses opens again after this long, while the item keeps its tab. */
const REOPEN_MS = 2000;

function hostOf(url: string | null | undefined): string | null {
  if (!url) return null;
  try {
    return new URL(url).host;
  } catch {
    return null;
  }
}

/** The item's title, else the last part of its page's path. */
function titleOf(item: RunItemView): string {
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
export function ItemTile({ item }: { item: RunItemView }) {
  const status = itemStatus(item);
  const title = titleOf(item);
  const site = hostOf(item.url) ?? item.site ?? "";
  const [refused, setRefused] = useState<string | null>(null);
  const live = !!item.tab_id && refused !== item.tab_id;

  return (
    <div className="flex w-[156px] shrink-0 flex-col gap-1.5">
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
            <TileScreen tabId={item.tab_id!} title={title} onRefused={() => setRefused(item.tab_id!)} />
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
    </div>
  );
}

/** The tab streamed at tile size; the Host sizes frames to the tile and leaves the page as it is. */
function TileScreen({ tabId, title, onRefused }: { tabId: string; title: string; onRefused(): void }) {
  const controller = useController();
  const canvas = useRef<HTMLCanvasElement>(null);
  const [attempt, setAttempt] = useState(0);
  const refusedRef = useRef(onRefused);
  refusedRef.current = onRefused;

  useEffect(() => {
    let gone = false;
    let streamId: string | null = null;
    let stream: BrowserStream | null = null;
    let reopen: ReturnType<typeof setTimeout> | null = null;
    let pending: ReturnType<typeof setTimeout> | null = null;
    let latest: BrowserScreenFrame | null = null;
    let lastPaint = 0;
    const scale = Math.min(2, Math.max(1, window.devicePixelRatio || 1));

    const paint = async () => {
      pending = null;
      const frame = latest;
      latest = null;
      if (!frame || gone) return;
      lastPaint = Date.now();
      try {
        const bitmap = await createImageBitmap(new Blob([frame.jpeg as BlobPart], { type: "image/jpeg" }));
        const c = canvas.current;
        if (!gone && c) {
          const w = Math.round(c.clientWidth * scale);
          const h = Math.round(c.clientHeight * scale);
          if (c.width !== w || c.height !== h) {
            c.width = w;
            c.height = h;
          }
          const ctx = c.getContext("2d");
          // Fill the width and keep the top of the page; what does not fit is cut at the bottom.
          const k = w / bitmap.width;
          ctx?.clearRect(0, 0, w, h);
          ctx?.drawImage(bitmap, 0, 0, w, bitmap.height * k);
        }
        bitmap.close();
      } catch {
        // A frame that does not decode is skipped; the next one replaces it.
      }
    };

    const onFrame = (frame: BrowserScreenFrame) => {
      latest = frame;
      if (pending) return;
      pending = setTimeout(() => void paint(), Math.max(0, lastPaint + PAINT_MS - Date.now()));
    };

    void (async () => {
      try {
        const id = await controller.openBrowserView(tabId, { width: WIDTH, height: HEIGHT, scale });
        if (gone) {
          void controller.closeBrowserView(id).catch(() => {});
          return;
        }
        streamId = id;
        stream = await controller.openBrowserStream(id, {
          onFrame,
          onClose: () => {
            stream = null;
            if (!gone) reopen = setTimeout(() => setAttempt((a) => a + 1), REOPEN_MS);
          },
        });
      } catch {
        // Refused (a paired device cannot watch the Host's tabs) or the tab is gone: show the card.
        if (!gone) refusedRef.current();
      }
    })();

    return () => {
      gone = true;
      if (reopen) clearTimeout(reopen);
      if (pending) clearTimeout(pending);
      stream?.close();
      stream = null;
      if (streamId) void controller.closeBrowserView(streamId).catch(() => {});
    };
  }, [controller, tabId, attempt]);

  return <canvas ref={canvas} className="size-full" aria-label={`Page: ${title}`} />;
}
