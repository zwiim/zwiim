// The Host's browser beside the conversation (design 19.1): the session's
// tabs and the person's own, an address bar, and the selected tab streamed
// from the Host's engine. Both the person and the session's agent drive a
// session tab; there is no control step between them. A page's own dialog
// (`alert`, `confirm`, `prompt`, leaving the page) is drawn over its tab.

import { useEffect, useMemo, useRef, useState } from "react";
import { ArrowLeft, ArrowRight, ExternalLink, Globe, Plus, RotateCw, X } from "lucide-react";
import {
  personalTabs,
  sessionTabs,
  tabUiRequests,
  ZwiimClientError,
  type BrowserEngine,
  type BrowserScreenFrame,
  type BrowserStream,
  type BrowserTab,
  type BrowserTabOwner,
  type BrowserUiAnswer,
  type BrowserUiRequest,
} from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import { openLink, isOpenableLink } from "@/lib/bridge";
import {
  addressUrl,
  browserShortcut,
  buttonOf,
  fitFrame,
  keyInput,
  modifiersOf,
  toPage,
  type BrowserShortcut,
  type Fit,
} from "@/lib/browser-input";
import { errorMessage, useController, useHost, useOnline } from "@/lib/host-context";
import { isModKey } from "@/lib/platform";
import { cn } from "@/lib/utils";

const CLOSE =
  "text-muted-foreground hover:text-foreground flex size-4 items-center justify-center rounded-sm transition-colors disabled:opacity-50";

/** How long a resize settles before the Host's tab takes the new size. */
const RESIZE_SETTLE_MS = 150;
/** Before a stream that ended on its own is opened again. */
const REOPEN_MS = 1000;

export function BrowserPanel({
  sessionId,
  active,
}: {
  /** The chat on screen: its tabs show first, and a new tab is its own. */
  sessionId: string | null;
  /** On screen; a hidden panel streams nothing. */
  active: boolean;
}) {
  const controller = useController();
  const online = useOnline();
  const browser = useHost((s) => s.browser);
  const [selected, setSelected] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const address = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (active) controller.watchBrowser();
  }, [active, controller]);

  const tabs = useMemo(
    () => [...(sessionId ? sessionTabs(browser, sessionId) : []), ...personalTabs(browser)],
    [browser, sessionId],
  );
  // The selection falls back to the first tab when its tab goes.
  const tab = tabs.find((t) => t.tab_id === selected) ?? tabs[0] ?? null;
  const owner: BrowserTabOwner = sessionId ? { kind: "session", session_id: sessionId } : { kind: "personal" };

  async function run(step: () => Promise<unknown>) {
    setError(null);
    try {
      await step();
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  const newTab = () =>
    run(async () => {
      const opened = await controller.openBrowserTab(owner);
      setSelected(opened.tab_id);
      requestAnimationFrame(() => address.current?.focus());
    });

  function shortcut(s: BrowserShortcut) {
    if (!online) return;
    switch (s) {
      case "address":
        address.current?.focus();
        address.current?.select();
        return;
      case "new_tab":
        void newTab();
        return;
      case "close_tab":
        if (tab) void run(() => controller.closeBrowserTab(tab.tab_id));
        return;
      case "reload":
        if (tab) void run(() => controller.browserTabHistory(tab.tab_id, "reload"));
        return;
      case "back":
      case "forward":
        if (tab) void run(() => controller.browserTabHistory(tab.tab_id, s));
        return;
    }
  }

  const unavailable = unavailableReason(browser.status, online);
  return (
    <div
      className="flex h-full min-h-0 flex-col"
      onKeyDown={(e) => {
        const s = browserShortcut(e);
        if (!s) return;
        e.preventDefault();
        e.stopPropagation();
        shortcut(s);
      }}
    >
      <div className="flex h-9 shrink-0 items-center gap-1 overflow-x-auto border-b px-2" role="tablist" aria-label="Browser tabs">
        {tabs.map((t) => (
          <div
            key={t.tab_id}
            className={cn(
              "flex max-w-44 shrink-0 items-center gap-1 rounded-md px-2 py-0.5 text-xs transition-colors",
              tab?.tab_id === t.tab_id ? "bg-accent font-medium" : "hover:bg-muted",
            )}
          >
            <button
              role="tab"
              className="min-w-0 truncate"
              aria-selected={tab?.tab_id === t.tab_id}
              title={t.url}
              onClick={() => setSelected(t.tab_id)}
            >
              {t.owner.kind === "personal" && <span className="text-muted-foreground">Mine · </span>}
              {tabLabel(t)}
            </button>
            {tab?.tab_id !== t.tab_id && tabUiRequests(browser, t.tab_id).length > 0 && (
              <span className="bg-brand size-1.5 shrink-0 rounded-full" title="This page is asking something" />
            )}
            <button
              className={CLOSE}
              title="Close tab"
              aria-label={`Close ${tabLabel(t)}`}
              disabled={!online}
              onClick={() => void run(() => controller.closeBrowserTab(t.tab_id))}
            >
              <X className="size-3" />
            </button>
          </div>
        ))}
        <Button size="icon-sm" variant="ghost" title="New tab" disabled={!online || !!unavailable} onClick={() => void newTab()}>
          <Plus />
        </Button>
      </div>
      {tab && (
        <AddressBar
          key={tab.tab_id}
          tab={tab}
          inputRef={address}
          disabled={!online}
          onGo={(url) => run(() => controller.navigateBrowserTab(tab.tab_id, url))}
          onStep={(action) => run(() => controller.browserTabHistory(tab.tab_id, action))}
        />
      )}
      {error && <p className="text-destructive border-b px-3 py-1.5 text-xs">{error}</p>}
      <div className="relative min-h-0 flex-1">
        {unavailable ? (
          <Notice>{unavailable}</Notice>
        ) : !tab ? (
          browser.status === "live" ? (
            <Notice>
              <Globe className="text-muted-foreground size-5" />
              <span>No tabs open.</span>
              <Button size="xs" variant="secondary" disabled={!online} onClick={() => void newTab()}>
                New tab
              </Button>
            </Notice>
          ) : null
        ) : (
          <>
            {active && !tab.discarded && browser.engine?.state === "running" && (
              <BrowserView key={tab.tab_id} tab={tab} onShortcut={shortcut} />
            )}
            <EngineNotice
              engine={browser.engine}
              tab={tab}
              onReset={() => run(() => controller.resetBrowserEngine())}
              onReload={() => run(() => controller.browserTabHistory(tab.tab_id, "reload"))}
            />
            {tabUiRequests(browser, tab.tab_id).slice(0, 1).map((r) => (
              <PageDialog
                key={r.request_id}
                request={r}
                disabled={!online}
                onAnswer={(answer) =>
                  run(async () => {
                    try {
                      await controller.answerBrowserUi(r.request_id, answer);
                    } catch (e) {
                      // Another client answered first, or it timed out: it is done.
                      if (!(e instanceof ZwiimClientError && e.code === "request_not_found")) throw e;
                    }
                  })
                }
              />
            ))}
          </>
        )}
      </div>
    </div>
  );
}

function unavailableReason(status: "off" | "loading" | "live" | "refused", online: boolean): string | null {
  if (status === "refused") return "The browser is only available on the host's own computer for now.";
  if (status === "off" && online) return "This host has no browser yet. Update Zwiim on the host to use it.";
  return null;
}

function tabLabel(t: BrowserTab): string {
  if (t.title) return t.title;
  if (t.url === "about:blank") return "New tab";
  try {
    return new URL(t.url).host || t.url;
  } catch {
    return t.url;
  }
}

function Notice({ children }: { children: React.ReactNode }) {
  return (
    <div className="text-muted-foreground text-ui absolute inset-0 flex flex-col items-center justify-center gap-2 p-6 text-center">
      {children}
    </div>
  );
}

/** What stands in for the page while the engine or the tab cannot show it. */
function EngineNotice({
  engine,
  tab,
  onReset,
  onReload,
}: {
  engine: BrowserEngine | null;
  tab: BrowserTab;
  onReset(): void;
  onReload(): void;
}) {
  if (engine?.state === "failed") {
    return (
      <Notice>
        <span className="text-destructive">The browser could not start.</span>
        <span className="text-xs">{engine.message}</span>
        <Button size="xs" variant="secondary" onClick={onReset}>
          Try again
        </Button>
      </Notice>
    );
  }
  if (engine?.state === "starting") {
    return <Notice>Starting the browser… The first start downloads it, which can take a minute.</Notice>;
  }
  if (tab.crashed) {
    return (
      <Notice>
        <span>This page crashed.</span>
        <Button size="xs" variant="secondary" onClick={onReload}>
          Reload
        </Button>
      </Notice>
    );
  }
  if (tab.discarded) {
    return (
      <Notice>
        <span>This tab was put to sleep to save memory.</span>
        <Button size="xs" variant="secondary" onClick={onReload}>
          Load it again
        </Button>
      </Notice>
    );
  }
  return null;
}

/** A page's JavaScript dialog, over its tab: the first answer from any client wins. */
function PageDialog({
  request,
  disabled,
  onAnswer,
}: {
  request: BrowserUiRequest;
  disabled: boolean;
  onAnswer(answer: BrowserUiAnswer): Promise<void>;
}) {
  const [text, setText] = useState(request.default_text ?? "");
  const [busy, setBusy] = useState(false);
  const leaving = request.dialog === "beforeunload";
  const prompt = request.dialog === "prompt";

  async function answer(accept: boolean) {
    if (busy || disabled) return;
    setBusy(true);
    try {
      await onAnswer({ kind: "dialog", accept, ...(prompt && accept ? { text } : {}) });
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="bg-scrim absolute inset-0 z-10 flex items-start justify-center p-6 pt-10">
      <form
        className="bg-popover ring-border shadow-pop presence-pop flex w-full max-w-sm flex-col gap-3 rounded-xl p-4 ring-1"
        role="alertdialog"
        aria-label={leaving ? "Leave this page?" : `${pageHost(request.url)} says`}
        onSubmit={(e) => {
          e.preventDefault();
          void answer(true);
        }}
        onKeyDown={(e) => {
          // The page is behind this: keys stay here.
          e.stopPropagation();
          if (e.key === "Escape") {
            e.preventDefault();
            void answer(false);
          }
        }}
      >
        <p className="text-ui font-medium">{leaving ? "Leave this page?" : `${pageHost(request.url)} says`}</p>
        <p className="text-ui max-h-48 overflow-y-auto break-words whitespace-pre-wrap">
          {leaving ? "Changes you made may not be saved." : request.message}
        </p>
        {prompt && (
          <input
            autoFocus
            className="border-border bg-input/30 text-ui h-8 rounded-md border px-2 outline-none focus-visible:ring-2"
            aria-label="Answer"
            value={text}
            onChange={(e) => setText(e.target.value)}
          />
        )}
        <div className="flex justify-end gap-2">
          {request.dialog !== "alert" && (
            <Button type="button" size="sm" variant="secondary" disabled={busy || disabled} onClick={() => void answer(false)}>
              {leaving ? "Stay" : "Cancel"}
            </Button>
          )}
          <Button type="submit" size="sm" autoFocus={!prompt} disabled={busy || disabled}>
            {leaving ? "Leave" : "OK"}
          </Button>
        </div>
      </form>
    </div>
  );
}

function pageHost(url: string): string {
  try {
    return new URL(url).host || "This page";
  } catch {
    return "This page";
  }
}

function AddressBar({
  tab,
  inputRef,
  disabled,
  onGo,
  onStep,
}: {
  tab: BrowserTab;
  inputRef: React.RefObject<HTMLInputElement | null>;
  disabled: boolean;
  onGo(url: string): Promise<unknown>;
  onStep(action: "back" | "forward" | "reload" | "stop"): Promise<unknown>;
}) {
  // What the person is typing; the tab's own URL whenever they are not.
  const [draft, setDraft] = useState<string | null>(null);
  return (
    <form
      className="flex h-9 shrink-0 items-center gap-1 border-b px-2"
      onSubmit={(e) => {
        e.preventDefault();
        const url = addressUrl(draft ?? tab.url);
        if (!url) return;
        setDraft(null);
        inputRef.current?.blur();
        void onGo(url);
      }}
    >
      <Button type="button" size="icon-sm" variant="ghost" title="Back" disabled={disabled} onClick={() => void onStep("back")}>
        <ArrowLeft />
      </Button>
      <Button type="button" size="icon-sm" variant="ghost" title="Forward" disabled={disabled} onClick={() => void onStep("forward")}>
        <ArrowRight />
      </Button>
      {tab.loading ? (
        <Button type="button" size="icon-sm" variant="ghost" title="Stop" disabled={disabled} onClick={() => void onStep("stop")}>
          <X />
        </Button>
      ) : (
        <Button type="button" size="icon-sm" variant="ghost" title="Reload" disabled={disabled} onClick={() => void onStep("reload")}>
          <RotateCw />
        </Button>
      )}
      <input
        ref={inputRef}
        className="bg-input/30 border-border focus-visible:ring-ring/50 h-7 min-w-0 flex-1 rounded-md border px-2 font-mono text-xs outline-none focus-visible:ring-2"
        aria-label="Address"
        spellCheck={false}
        autoComplete="off"
        disabled={disabled}
        value={draft ?? (tab.url === "about:blank" ? "" : tab.url)}
        placeholder="Search or enter an address"
        onChange={(e) => setDraft(e.target.value)}
        onFocus={(e) => e.target.select()}
        onKeyDown={(e) => {
          if (e.key !== "Escape") return;
          setDraft(null);
          e.currentTarget.blur();
        }}
        onBlur={() => setDraft((d) => (d === null || d.trim() === "" ? null : d))}
      />
      <Button
        type="button"
        size="icon-sm"
        variant="ghost"
        title="Open in my browser"
        disabled={!isOpenableLink(tab.url)}
        onClick={() => void openLink(tab.url)}
      >
        <ExternalLink />
      </Button>
    </form>
  );
}

/** The viewport the Host's tab takes for a box this size. */
function viewportOf(box: { width: number; height: number }) {
  return {
    width: Math.min(8192, Math.max(100, Math.round(box.width))),
    height: Math.min(8192, Math.max(100, Math.round(box.height))),
    scale: Math.min(4, Math.max(0.5, window.devicePixelRatio || 1)),
  };
}

/**
 * The selected tab, streamed. The canvas takes the page's pointer and keys;
 * the panel's shortcuts pass through to the panel.
 */
export function BrowserView({ tab, onShortcut }: { tab: BrowserTab; onShortcut(s: BrowserShortcut): void }) {
  const controller = useController();
  const box = useRef<HTMLDivElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const stream = useRef<BrowserStream | null>(null);
  // The last frame's page size and where it was drawn, for mapping the pointer.
  const page = useRef({ width: 0, height: 0 });
  const fit = useRef<Fit>({ x: 0, y: 0, width: 0, height: 0 });
  const [attempt, setAttempt] = useState(0);
  const [failure, setFailure] = useState<string | null>(null);
  const [painted, setPainted] = useState(false);

  useEffect(() => {
    const el = box.current;
    if (!el) return;
    let gone = false;
    let streamId: string | null = null;
    let reopen: ReturnType<typeof setTimeout> | null = null;
    let latest: BrowserScreenFrame | null = null;
    let decoding = false;

    const paint = (bitmap: ImageBitmap, frame: BrowserScreenFrame) => {
      const c = canvas.current;
      if (!c) return;
      const dpr = window.devicePixelRatio || 1;
      const rect = { width: el.clientWidth, height: el.clientHeight };
      const w = Math.round(rect.width * dpr);
      const h = Math.round(rect.height * dpr);
      if (c.width !== w || c.height !== h) {
        c.width = w;
        c.height = h;
      }
      page.current = { width: frame.css_width, height: frame.css_height };
      fit.current = fitFrame(rect, page.current);
      const ctx = c.getContext("2d");
      if (!ctx) return;
      ctx.clearRect(0, 0, w, h);
      const f = fit.current;
      ctx.drawImage(bitmap, f.x * dpr, f.y * dpr, f.width * dpr, f.height * dpr);
    };

    // Newest frame wins: one decode at a time, frames that arrive meanwhile replace each other.
    const show = async (frame: BrowserScreenFrame) => {
      latest = frame;
      if (decoding) return;
      decoding = true;
      while (latest && !gone) {
        const next: BrowserScreenFrame = latest;
        latest = null;
        try {
          const bitmap = await createImageBitmap(new Blob([next.jpeg as BlobPart], { type: "image/jpeg" }));
          if (!gone) paint(bitmap, next);
          bitmap.close();
          if (!gone) setPainted(true);
        } catch {
          // A frame that does not decode is skipped; the next one replaces it.
        }
      }
      decoding = false;
    };

    void (async () => {
      try {
        const id = await controller.openBrowserView(tab.tab_id, viewportOf({ width: el.clientWidth, height: el.clientHeight }));
        if (gone) {
          void controller.closeBrowserView(id).catch(() => {});
          return;
        }
        streamId = id;
        stream.current = await controller.openBrowserStream(id, {
          onOpen: () => setFailure(null),
          onFrame: (frame) => void show(frame),
          onClose: () => {
            stream.current = null;
            // The tab's engine restarted or the stream lapsed: watch again shortly.
            if (!gone) reopen = setTimeout(() => setAttempt((a) => a + 1), REOPEN_MS);
          },
        });
      } catch (e) {
        if (!gone) setFailure(errorMessage(e));
      }
    })();

    let settle: ReturnType<typeof setTimeout> | null = null;
    const resized = new ResizeObserver(() => {
      if (settle) clearTimeout(settle);
      settle = setTimeout(() => {
        stream.current?.send({ type: "viewport", ...viewportOf({ width: el.clientWidth, height: el.clientHeight }) });
      }, RESIZE_SETTLE_MS);
    });
    resized.observe(el);

    return () => {
      gone = true;
      resized.disconnect();
      if (settle) clearTimeout(settle);
      if (reopen) clearTimeout(reopen);
      stream.current?.close();
      stream.current = null;
      if (streamId) void controller.closeBrowserView(streamId).catch(() => {});
    };
  }, [controller, tab.tab_id, attempt]);

  // Moves are sent at most once per animation frame (design 7.6).
  const move = useRef<{ x: number; y: number; buttons: number; modifiers: number } | null>(null);
  const moveFrame = useRef<number | null>(null);
  useEffect(
    () => () => {
      if (moveFrame.current !== null) cancelAnimationFrame(moveFrame.current);
    },
    [],
  );

  /** The pointer as page CSS pixels; clamped to the page when `clamp`, else null outside it. */
  function pointOf(e: React.PointerEvent | React.WheelEvent, clamp = false) {
    const rect = canvas.current!.getBoundingClientRect();
    const local = { x: e.clientX - rect.left, y: e.clientY - rect.top };
    const p = toPage(local, fit.current, page.current);
    if (p || !clamp || fit.current.width <= 0) return p;
    const f = fit.current;
    const x = Math.min(f.x + f.width, Math.max(f.x, local.x));
    const y = Math.min(f.y + f.height, Math.max(f.y, local.y));
    return toPage({ x, y }, f, page.current);
  }

  const send = (input: Parameters<BrowserStream["send"]>[0]) => stream.current?.send(input);

  return (
    <div ref={box} className="bg-background absolute inset-0">
      <canvas
        ref={canvas}
        className="focus-visible:ring-brand/55 absolute inset-0 size-full outline-none focus-visible:ring-2"
        tabIndex={0}
        aria-label={`Page: ${tab.title || tab.url}`}
        onContextMenu={(e) => e.preventDefault()}
        onPointerDown={(e) => {
          e.currentTarget.focus();
          const p = pointOf(e);
          if (!p) return;
          e.currentTarget.setPointerCapture(e.pointerId);
          const button = buttonOf(e.button);
          send({ type: "mouse", action: "move", ...p, buttons: e.buttons, modifiers: modifiersOf(e) });
          send({ type: "mouse", action: "down", ...p, button, buttons: e.buttons, click_count: Math.max(1, e.detail), modifiers: modifiersOf(e) });
        }}
        onPointerUp={(e) => {
          const p = pointOf(e, true);
          if (!p) return;
          send({ type: "mouse", action: "up", ...p, button: buttonOf(e.button), buttons: e.buttons, click_count: Math.max(1, e.detail), modifiers: modifiersOf(e) });
        }}
        onPointerMove={(e) => {
          const p = pointOf(e, e.buttons !== 0);
          if (!p) return;
          move.current = { ...p, buttons: e.buttons, modifiers: modifiersOf(e) };
          if (moveFrame.current !== null) return;
          moveFrame.current = requestAnimationFrame(() => {
            moveFrame.current = null;
            const m = move.current;
            move.current = null;
            if (m) send({ type: "mouse", action: "move", ...m });
          });
        }}
        onWheel={(e) => {
          const p = pointOf(e);
          if (!p) return;
          // Lines and pages as pixels, as Chromium counts them.
          const unit = e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? page.current.height : 1;
          send({ type: "mouse", action: "wheel", ...p, delta_x: e.deltaX * unit, delta_y: e.deltaY * unit, modifiers: modifiersOf(e) });
        }}
        onKeyDown={(e) => {
          const s = browserShortcut(e);
          if (s) {
            e.preventDefault();
            e.stopPropagation();
            onShortcut(s);
            return;
          }
          // Paste arrives as the paste event's text, not as the key.
          if (isModKey(e) && e.key.toLowerCase() === "v") return;
          e.preventDefault();
          send(keyInput(e, "down"));
        }}
        onKeyUp={(e) => {
          if (browserShortcut(e) || (isModKey(e) && e.key.toLowerCase() === "v")) return;
          e.preventDefault();
          send(keyInput(e, "up"));
        }}
        onPaste={(e) => {
          const text = e.clipboardData.getData("text/plain");
          e.preventDefault();
          if (text) send({ type: "text", text });
        }}
      />
      {!painted && !failure && <Notice>Loading…</Notice>}
      {failure && <Notice>{failure}</Notice>}
    </div>
  );
}
