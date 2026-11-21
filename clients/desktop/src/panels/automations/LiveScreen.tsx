import { useEffect, useRef, useState } from "react";
import type { BrowserScreenFrame, BrowserStream } from "@zwiim/sdk";
import { useController } from "@/lib/host-context";

/** A stream that lapses opens again after this long, while the item keeps its tab. */
const REOPEN_MS = 2000;

/**
 * A tab streamed at `viewport`: the Host sizes frames to it and leaves the
 * page as it is. Paints at most every `paintMs`, the newest frame winning; a
 * lapsed stream opens again after a moment, a refused one calls `onRefused`.
 */
export function LiveScreen({
  tabId,
  title,
  viewport,
  paintMs,
  onRefused,
}: {
  tabId: string;
  title: string;
  viewport: { width: number; height: number };
  paintMs: number;
  onRefused(): void;
}) {
  const controller = useController();
  const canvas = useRef<HTMLCanvasElement>(null);
  const [attempt, setAttempt] = useState(0);
  const refusedRef = useRef(onRefused);
  refusedRef.current = onRefused;

  // The viewport is read once per open: a tile and the focus view each keep theirs.
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
      pending = setTimeout(() => void paint(), Math.max(0, lastPaint + paintMs - Date.now()));
    };

    void (async () => {
      try {
        const id = await controller.openBrowserView(tabId, { width: viewport.width, height: viewport.height, scale });
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
