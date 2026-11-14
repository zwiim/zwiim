// The Host's browser beside the conversation (design 19.1): the session's
// tabs and the person's own, an address bar, and the selected tab streamed
// from the Host's engine. Both the person and the session's agent drive a
// session tab; there is no control step between them. What a page asks of a
// person (its `alert`, `confirm`, `prompt` or leave-page dialog, a site's
// sign-in, a file input's picker of workspace files) is drawn over its tab,
// and so are what the engine does not paint: the page's menu, a select's
// list, a date picker, and find in page.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ArrowLeft,
  ArrowRight,
  ChevronLeft,
  ExternalLink,
  File,
  Folder,
  Globe,
  Maximize2,
  Minimize2,
  MousePointer2,
  Plus,
  Printer,
  RefreshCcwDot,
  RotateCw,
  SquareDashedMousePointer,
  X,
} from "lucide-react";
import {
  personalTabs,
  sessionTabs,
  tabAgentAction,
  tabMenu,
  tabPick,
  tabUiRequests,
  uploadAttachment,
  ZwiimClientError,
  type BrowserAgentAction,
  type BrowserEngine,
  type BrowserMenu,
  type BrowserPreviewId,
  type BrowserScreenFrame,
  type BrowserStream,
  type BrowserTab,
  type BrowserTabOwner,
  type BrowserUiAnswer,
  type BrowserUiRequest,
  type FileTreeNode,
} from "@zwiim/sdk";
import { readBase64 } from "@/attachments/usePendingAttachments";
import { Button } from "@/components/ui/button";
import { openLink, isOpenableLink } from "@/lib/bridge";
import {
  addressUrl,
  browserShortcut,
  buttonOf,
  fitFrame,
  fromPage,
  keyInput,
  modifiersOf,
  toPage,
  type BrowserShortcut,
  type Fit,
} from "@/lib/browser-input";
import { draggedFile, type DraggedFile } from "@/lib/file-drag";
import { errorMessage, useController, useHost, useOnline } from "@/lib/host-context";
import { handToChat } from "@/lib/page-handoff";
import { isModKey } from "@/lib/platform";
import { DatePicker, FindBar, isPick, PageMenu, SelectPicker, type MenuAction, type PickRequest } from "@/panels/BrowserPageUi";
import { BrowserLogin } from "@/panels/BrowserLogin";
import { BrowserSites } from "@/panels/BrowserSites";
import { BrowserSteps } from "@/panels/BrowserSteps";
import { BrowserPreview } from "@/panels/BrowserPreview";
import { cn } from "@/lib/utils";

const CLOSE =
  "text-muted-foreground hover:text-foreground flex size-4 items-center justify-center rounded-sm transition-colors disabled:opacity-50";

/** How long a resize settles before the Host's tab takes the new size. */
const RESIZE_SETTLE_MS = 150;
/** Before a stream that ended on its own is opened again. */
const REOPEN_MS = 1000;

export function BrowserPanel({
  workspaceId,
  sessionId,
  active,
  wide = false,
  onWide,
}: {
  /** The workspace on screen: where a page's file input picks from. */
  workspaceId: string;
  /** The chat on screen: its tabs show first, and a new tab is its own. */
  sessionId: string | null;
  /** On screen; a hidden panel streams nothing. */
  active: boolean;
  /** Filling the window beside a narrow chat. */
  wide?: boolean;
  /** Expand over the chat, or shrink back. */
  onWide?(): void;
}) {
  const controller = useController();
  const online = useOnline();
  const browser = useHost((s) => s.browser);
  const [selected, setSelected] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // The PDF just saved, said until the next step.
  const [saved, setSaved] = useState<string | null>(null);
  const [finding, setFinding] = useState(false);
  // The tab an element pick is armed in, and its last ended pick's seq then.
  const [picking, setPicking] = useState<{ tabId: string; since: number } | null>(null);
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
  const tabId = tab?.tab_id ?? null;
  useEffect(() => setFinding(false), [tabId]);
  // A pick ends with the tab's next `browser.tab.picked`; its element goes
  // to this chat's next message (design 10.6).
  const ended = picking ? tabPick(browser, picking.tabId) : undefined;
  useEffect(() => {
    if (!picking || !ended || ended.seq <= picking.since) return;
    setPicking(null);
    if (ended.pick && sessionId) {
      handToChat(sessionId, { label: ended.pick.label, text: ended.pick.text });
      setSaved(`Added ${ended.pick.label} to your next message.`);
    }
  }, [picking, ended, sessionId]);
  // Leaving the tab, or the panel, calls its pick off.
  useEffect(() => {
    if (!picking || (picking.tabId === tabId && active)) return;
    void controller.cancelBrowserPick(picking.tabId).catch(() => {});
    setPicking(null);
  }, [picking, tabId, active, controller]);
  const find = useCallback(
    (query: string, backwards: boolean) =>
      tabId ? controller.findInBrowserTab(tabId, query, backwards).catch(() => null) : Promise.resolve(null),
    [controller, tabId],
  );
  const stopFinding = () => {
    setFinding(false);
    void find("", false);
  };

  async function run(step: () => Promise<unknown>) {
    setError(null);
    setSaved(null);
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

  const togglePick = (t: BrowserTab) =>
    run(async () => {
      if (picking?.tabId === t.tab_id) {
        setPicking(null);
        await controller.cancelBrowserPick(t.tab_id);
        return;
      }
      const since = tabPick(browser, t.tab_id)?.seq ?? 0;
      await controller.pickInBrowserTab(t.tab_id);
      setPicking({ tabId: t.tab_id, since });
    });

  const print = (t: BrowserTab) =>
    run(async () => {
      const name = await controller.printBrowserTab(t.tab_id);
      setSaved(t.owner.kind === "session" ? `Saved ${name} to this chat's downloads.` : `Saved ${name} to Downloads.`);
    });

  // Files from this computer reach the page through the workspace: uploaded
  // first, then dropped by id (design 7.6).
  const dropFiles = (t: BrowserTab, at: { x: number; y: number }, files: File[]) =>
    run(async () => {
      const ids: string[] = [];
      for (const f of files) {
        const up = await uploadAttachment(controller.client, workspaceId, f.name, await readBase64(f));
        ids.push(up.attachment_id);
      }
      await controller.dropOnBrowserTab(t.tab_id, at.x, at.y, workspaceId, { attachment_ids: ids });
    });

  // Files from the Files panel are in the workspace already: dropped by path.
  const dropPath = (t: BrowserTab, at: { x: number; y: number }, file: DraggedFile) =>
    run(() => controller.dropOnBrowserTab(t.tab_id, at.x, at.y, file.workspaceId, { paths: [file.path] }));

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
      case "find":
        if (tab) setFinding(true);
        return;
      case "print":
        if (tab) void print(tab);
        return;
    }
  }

  const answerUi = (r: BrowserUiRequest, answer: BrowserUiAnswer) =>
    run(async () => {
      try {
        await controller.answerBrowserUi(r.request_id, answer);
      } catch (e) {
        // Another client answered first, or it timed out: it is done.
        if (!(e instanceof ZwiimClientError && e.code === "request_not_found")) throw e;
      }
    });

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
      <div className="flex h-9 shrink-0 items-center border-b pr-2">
        <div className="flex h-full min-w-0 flex-1 items-center gap-1 overflow-x-auto px-2" role="tablist" aria-label="Browser tabs">
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
        {sessionId && !unavailable && <BrowserSteps key={`steps:${sessionId}`} sessionId={sessionId} disabled={!online} />}
        {sessionId && !unavailable && <BrowserLogin key={`login:${sessionId}`} sessionId={sessionId} disabled={!online} />}
        {sessionId && !unavailable && <BrowserSites key={`sites:${sessionId}`} sessionId={sessionId} disabled={!online} />}
        {onWide && (
          <Button
            size="icon-sm"
            variant={wide ? "secondary" : "ghost"}
            title={wide ? "Shrink the browser" : "Expand the browser beside the chat"}
            aria-pressed={wide}
            onClick={onWide}
          >
            {wide ? <Minimize2 /> : <Maximize2 />}
          </Button>
        )}
      </div>
      {tab && (
        <AddressBar
          key={tab.tab_id}
          tab={tab}
          inputRef={address}
          disabled={!online}
          onGo={(url) => run(() => controller.navigateBrowserTab(tab.tab_id, url))}
          onStep={(action) => run(() => controller.browserTabHistory(tab.tab_id, action))}
          onPrint={() => void print(tab)}
          // Only a chat takes what is picked.
          onPick={sessionId ? () => void togglePick(tab) : null}
          picking={picking?.tabId === tab.tab_id}
          onReloadOnSave={
            tab.owner.kind === "session" && tab.reload_on_save !== undefined
              ? () => void run(() => controller.setBrowserReloadOnSave(tab.tab_id, !tab.reload_on_save))
              : null
          }
          onPreview={(preview) => void run(() => controller.setBrowserPreview(tab.tab_id, preview))}
        />
      )}
      {picking && picking.tabId === tab?.tab_id && (
        <p className="text-muted-foreground border-b px-3 py-1.5 text-xs" role="status">
          Click an element on the page to add it to your next message. Esc cancels.
        </p>
      )}
      {error && <p className="text-destructive border-b px-3 py-1.5 text-xs">{error}</p>}
      {saved && (
        <p className="text-muted-foreground border-b px-3 py-1.5 text-xs" role="status">
          {saved}
        </p>
      )}
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
              <BrowserView
                key={tab.tab_id}
                tab={tab}
                agent={tabAgentAction(browser, tab.tab_id)}
                menu={tabMenu(browser, tab.tab_id)}
                pick={tabUiRequests(browser, tab.tab_id).find(isPick)}
                disabled={!online}
                onShortcut={shortcut}
                onDropFiles={(at, files) => void dropFiles(tab, at, files)}
                onDropPath={(at, file) => void dropPath(tab, at, file)}
                onAnswer={(r, answer) => void answerUi(r, answer)}
                onMenu={(a) => {
                  if (a.kind === "step") void run(() => controller.browserTabHistory(tab.tab_id, a.action));
                  else if (a.kind === "open")
                    void run(async () => {
                      const opened = await controller.openBrowserTab(tab.owner, a.url);
                      setSelected(opened.tab_id);
                    });
                }}
              />
            )}
            {finding && <FindBar key={tab.tab_id} find={find} onClose={stopFinding} />}
            <EngineNotice
              engine={browser.engine}
              tab={tab}
              onReset={() => run(() => controller.resetBrowserEngine())}
              onReload={() => run(() => controller.browserTabHistory(tab.tab_id, "reload"))}
            />
            {tabUiRequests(browser, tab.tab_id)
              .filter((r) => !isPick(r))
              .slice(0, 1)
              .map((r) => (
                <PageRequest
                  key={r.request_id}
                  request={r}
                  workspaceId={workspaceId}
                  disabled={!online}
                  onAnswer={(answer) => answerUi(r, answer)}
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

type Answer = (answer: BrowserUiAnswer) => Promise<void>;
type DialogRequest = Extract<BrowserUiRequest, { kind: "dialog" }>;
type AuthRequest = Extract<BrowserUiRequest, { kind: "auth" }>;
type ChooserRequest = Extract<BrowserUiRequest, { kind: "file_chooser" }>;
type SiteRequest = Extract<BrowserUiRequest, { kind: "site" }>;

/** What a tab waits on, over the tab: the first answer from any client wins. */
function PageRequest({
  request,
  workspaceId,
  disabled,
  onAnswer,
}: {
  request: BrowserUiRequest;
  workspaceId: string;
  disabled: boolean;
  onAnswer: Answer;
}) {
  switch (request.kind) {
    case "dialog":
      return <PageDialog request={request} disabled={disabled} onAnswer={onAnswer} />;
    case "auth":
      return <SignIn request={request} disabled={disabled} onAnswer={onAnswer} />;
    case "file_chooser":
      return <FileChooser request={request} workspaceId={workspaceId} disabled={disabled} onAnswer={onAnswer} />;
    case "site":
      return <SiteQuestion request={request} disabled={disabled} onAnswer={onAnswer} />;
    // Drawn by the view, where the element is.
    case "select":
    case "date":
      return null;
  }
}

/** One answer at a time. */
function useAnswer(disabled: boolean, onAnswer: Answer) {
  const [busy, setBusy] = useState(false);
  async function send(answer: BrowserUiAnswer) {
    if (busy || disabled) return;
    setBusy(true);
    try {
      await onAnswer(answer);
    } finally {
      setBusy(false);
    }
  }
  return { busy: busy || disabled, send };
}

/** The sheet over the page: Enter accepts, Escape cancels, and keys never reach the page. */
function Sheet({
  label,
  onAccept,
  onCancel,
  children,
}: {
  label: string;
  onAccept(): void;
  onCancel(): void;
  children: React.ReactNode;
}) {
  return (
    <div className="bg-scrim absolute inset-0 z-10 flex items-start justify-center p-6 pt-10">
      <form
        className="bg-popover ring-border shadow-pop presence-pop flex w-full max-w-sm flex-col gap-3 rounded-xl p-4 ring-1"
        role="alertdialog"
        aria-label={label}
        onSubmit={(e) => {
          e.preventDefault();
          onAccept();
        }}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Escape") {
            e.preventDefault();
            onCancel();
          }
        }}
      >
        <p className="text-ui font-medium">{label}</p>
        {children}
      </form>
    </div>
  );
}

const FIELD = "border-border bg-input/30 text-ui h-8 rounded-md border px-2 outline-none focus-visible:ring-2";

/** A page's `alert`, `confirm`, `prompt` or leave-page question. */
function PageDialog({ request, disabled, onAnswer }: { request: DialogRequest; disabled: boolean; onAnswer: Answer }) {
  const [text, setText] = useState(request.default_text ?? "");
  const { busy, send } = useAnswer(disabled, onAnswer);
  const leaving = request.dialog === "beforeunload";
  const prompt = request.dialog === "prompt";
  const answer = (accept: boolean) => void send({ kind: "dialog", accept, ...(prompt && accept ? { text } : {}) });

  return (
    <Sheet
      label={leaving ? "Leave this page?" : `${pageHost(request.url)} says`}
      onAccept={() => answer(true)}
      onCancel={() => answer(false)}
    >
      <p className="text-ui max-h-48 overflow-y-auto break-words whitespace-pre-wrap">
        {leaving ? "Changes you made may not be saved." : request.message}
      </p>
      {prompt && <input autoFocus className={FIELD} aria-label="Answer" value={text} onChange={(e) => setText(e.target.value)} />}
      <div className="flex justify-end gap-2">
        {request.dialog !== "alert" && (
          <Button type="button" size="sm" variant="secondary" disabled={busy} onClick={() => answer(false)}>
            {leaving ? "Stay" : "Cancel"}
          </Button>
        )}
        <Button type="submit" size="sm" autoFocus={!prompt} disabled={busy}>
          {leaving ? "Leave" : "OK"}
        </Button>
      </div>
    </Sheet>
  );
}

/** The agent's page wants a site nobody allowed for the session; the page waits. */
function SiteQuestion({ request, disabled, onAnswer }: { request: SiteRequest; disabled: boolean; onAnswer: Answer }) {
  const { busy, send } = useAnswer(disabled, onAnswer);
  const deny = () => void send({ kind: "site", allow: false });
  const site = request.site.replace(/:(80|443)$/, "");

  return (
    <Sheet label={`Let the agent open ${site}?`} onAccept={() => void send({ kind: "site", allow: true })} onCancel={deny}>
      <p className="text-ui text-muted-foreground break-all">{request.url}</p>
      {request.local && (
        <p className="text-ui">This is on your computer or local network, where admin pages and other services run.</p>
      )}
      <div className="flex justify-end gap-2">
        <Button type="button" size="sm" variant="secondary" disabled={busy} onClick={deny}>
          Deny
        </Button>
        <Button
          type="button"
          size="sm"
          variant="secondary"
          disabled={busy}
          onClick={() => void send({ kind: "site", allow: true, always: true })}
        >
          Always allow
        </Button>
        <Button type="submit" size="sm" autoFocus disabled={busy}>
          Allow
        </Button>
      </div>
    </Sheet>
  );
}

/** A site (or a proxy) asks for a username and password. They go to the Host's browser and nowhere else. */
function SignIn({ request, disabled, onAnswer }: { request: AuthRequest; disabled: boolean; onAnswer: Answer }) {
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const { busy, send } = useAnswer(disabled, onAnswer);
  const cancel = () => void send({ kind: "auth", accept: false });

  return (
    <Sheet
      label={request.proxy ? "Sign in to the proxy" : "Sign in"}
      onAccept={() => void send({ kind: "auth", accept: true, username, password })}
      onCancel={cancel}
    >
      <p className="text-ui text-muted-foreground break-words">
        {request.origin} asks for a username and password{request.realm ? ` for “${request.realm}”` : ""}.
      </p>
      <input
        autoFocus
        className={FIELD}
        aria-label="Username"
        placeholder="Username"
        autoComplete="off"
        value={username}
        onChange={(e) => setUsername(e.target.value)}
      />
      <input
        type="password"
        className={FIELD}
        aria-label="Password"
        placeholder="Password"
        autoComplete="off"
        value={password}
        onChange={(e) => setPassword(e.target.value)}
      />
      <div className="flex justify-end gap-2">
        <Button type="button" size="sm" variant="secondary" disabled={busy} onClick={cancel}>
          Cancel
        </Button>
        <Button type="submit" size="sm" disabled={busy}>
          Sign in
        </Button>
      </div>
    </Sheet>
  );
}

/**
 * A page's file input: the workspace's files, one folder at a time. The Host
 * hands the page only files inside the workspace.
 */
function FileChooser({
  request,
  workspaceId,
  disabled,
  onAnswer,
}: {
  request: ChooserRequest;
  workspaceId: string;
  disabled: boolean;
  onAnswer: Answer;
}) {
  const controller = useController();
  const [folder, setFolder] = useState("");
  const [nodes, setNodes] = useState<FileTreeNode[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [picked, setPicked] = useState<string[]>([]);
  const { busy, send } = useAnswer(disabled, onAnswer);

  useEffect(() => {
    let live = true;
    setNodes(null);
    setError(null);
    controller.client
      .request("workspace.tree", { workspace_id: workspaceId, ...(folder ? { path: folder } : {}), max_depth: 1 })
      .then((n) => live && setNodes([...n].sort((a, b) => (a.kind === b.kind ? a.name.localeCompare(b.name) : a.kind === "dir" ? -1 : 1))))
      .catch((e) => live && setError(errorMessage(e)));
    return () => {
      live = false;
    };
  }, [controller, workspaceId, folder]);

  const toggle = (path: string) =>
    setPicked((p) => (p.includes(path) ? p.filter((x) => x !== path) : request.multiple ? [...p, path] : [path]));
  const cancel = () => void send({ kind: "file_chooser", accept: false });
  const up = folder.includes("/") ? folder.slice(0, folder.lastIndexOf("/")) : "";

  return (
    <Sheet
      label={request.multiple ? "Choose files" : "Choose a file"}
      onAccept={() => {
        if (picked.length > 0) void send({ kind: "file_chooser", accept: true, workspace_id: workspaceId, paths: picked });
      }}
      onCancel={cancel}
    >
      <div className="text-muted-foreground flex h-6 items-center gap-1 text-xs">
        {folder && (
          <button type="button" className={CLOSE} title="Up" aria-label="Up" onClick={() => setFolder(up)}>
            <ChevronLeft className="size-3.5" />
          </button>
        )}
        <span className="truncate font-mono">{folder || "Workspace"}</span>
      </div>
      <ul className="border-border -mx-1 max-h-64 min-h-24 overflow-y-auto rounded-md border p-1" aria-label="Files">
        {error ? (
          <li className="text-destructive text-ui p-2">{error}</li>
        ) : !nodes ? null : nodes.length === 0 ? (
          <li className="text-muted-foreground text-ui p-2">This folder is empty.</li>
        ) : (
          nodes.map((n) => (
            <li key={n.path}>
              <button
                type="button"
                className={cn(
                  "text-ui flex w-full items-center gap-2 rounded px-2 py-1 text-left transition-colors",
                  picked.includes(n.path) ? "bg-accent font-medium" : "hover:bg-muted",
                )}
                aria-pressed={n.kind === "file" ? picked.includes(n.path) : undefined}
                onClick={() => (n.kind === "dir" ? setFolder(n.path) : toggle(n.path))}
                onDoubleClick={() => {
                  if (n.kind === "file" && !request.multiple)
                    void send({ kind: "file_chooser", accept: true, workspace_id: workspaceId, paths: [n.path] });
                }}
              >
                {n.kind === "dir" ? (
                  <Folder className="text-muted-foreground size-3.5 shrink-0" />
                ) : (
                  <File className="text-muted-foreground size-3.5 shrink-0" />
                )}
                <span className="truncate">{n.name}</span>
              </button>
            </li>
          ))
        )}
      </ul>
      <div className="flex items-center justify-end gap-2">
        {picked.length > 1 && <span className="text-muted-foreground mr-auto text-xs">{picked.length} files</span>}
        <Button type="button" size="sm" variant="secondary" disabled={busy} onClick={cancel}>
          Cancel
        </Button>
        <Button type="submit" size="sm" disabled={busy || picked.length === 0}>
          Choose
        </Button>
      </div>
    </Sheet>
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
  onPrint,
  onPick,
  picking,
  onReloadOnSave,
  onPreview,
}: {
  tab: BrowserTab;
  inputRef: React.RefObject<HTMLInputElement | null>;
  disabled: boolean;
  onGo(url: string): Promise<unknown>;
  onStep(action: "back" | "forward" | "reload" | "stop"): Promise<unknown>;
  onPrint(): void;
  /** Pick an element for the chat; null without a chat. */
  onPick: (() => void) | null;
  picking: boolean;
  /** Turn reload on save on or off; null for a tab without a worktree, or an older Host. */
  onReloadOnSave: (() => void) | null;
  onPreview(preview: BrowserPreviewId | null): void;
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
        variant={picking ? "secondary" : "ghost"}
        title={onPick ? (picking ? "Stop picking" : "Pick an element for the chat") : "Open a chat to pick an element for it"}
        aria-pressed={picking}
        disabled={disabled || !onPick}
        onClick={() => onPick?.()}
      >
        <SquareDashedMousePointer />
      </Button>
      {onReloadOnSave && (
        <Button
          type="button"
          size="icon-sm"
          variant={tab.reload_on_save ? "secondary" : "ghost"}
          title={tab.reload_on_save ? "Reloading when files change: turn off" : "Reload when files in this chat's worktree change"}
          aria-pressed={!!tab.reload_on_save}
          disabled={disabled}
          onClick={onReloadOnSave}
        >
          <RefreshCcwDot />
        </Button>
      )}
      <BrowserPreview tab={tab} disabled={disabled} onChoose={onPreview} />
      <Button type="button" size="icon-sm" variant="ghost" title="Save as PDF" disabled={disabled} onClick={onPrint}>
        <Printer />
      </Button>
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

/** Room left round a previewed device, for its outline. */
const PREVIEW_MARGIN = 16;
const shrink = (box: { width: number; height: number }) => ({
  width: Math.max(0, box.width - 2 * PREVIEW_MARGIN),
  height: Math.max(0, box.height - 2 * PREVIEW_MARGIN),
});
const inset = (f: Fit): Fit => ({ ...f, x: f.x + PREVIEW_MARGIN, y: f.y + PREVIEW_MARGIN });

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
export function BrowserView({
  tab,
  agent,
  menu,
  pick,
  disabled = false,
  onShortcut,
  onDropFiles,
  onDropPath,
  onAnswer,
  onMenu,
}: {
  tab: BrowserTab;
  /** The agent's latest action on this tab, shown as a ghost cursor. */
  agent?: { action: BrowserAgentAction; seq: number };
  /** The page's latest context menu; shown when a new one arrives. */
  menu?: { menu: BrowserMenu; seq: number };
  /** A select's list or a date picker the page waits on. */
  pick?: PickRequest;
  disabled?: boolean;
  onShortcut(s: BrowserShortcut): void;
  /** Files dropped on the page, at a point in its CSS pixels. */
  onDropFiles?(at: { x: number; y: number }, files: File[]): void;
  /** A workspace file dragged from the Files panel and dropped on the page. */
  onDropPath?(at: { x: number; y: number }, file: DraggedFile): void;
  onAnswer?(request: PickRequest, answer: BrowserUiAnswer): void;
  /** A menu pick the panel does: history and new tabs. */
  onMenu?(action: MenuAction): void;
}) {
  const controller = useController();
  const box = useRef<HTMLDivElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const stream = useRef<BrowserStream | null>(null);
  // The last frame's page size and where it was drawn, for mapping the pointer.
  const page = useRef({ width: 0, height: 0 });
  const fit = useRef<Fit>({ x: 0, y: 0, width: 0, height: 0 });
  // Where the frame sits, for the device outline of a previewed tab.
  const [frameBox, setFrameBox] = useState<Fit | null>(null);
  const previewed = useRef(!!tab.preview);
  previewed.current = !!tab.preview;
  const [attempt, setAttempt] = useState(0);
  const [failure, setFailure] = useState<string | null>(null);
  const [painted, setPainted] = useState(false);
  // Only a menu asked for while this view is up.
  const firstMenu = useRef(menu?.seq);
  const [menuShown, setMenuShown] = useState<number | null>(null);
  useEffect(() => {
    if (menu && menu.seq !== firstMenu.current) setMenuShown(menu.seq);
  }, [menu]);
  const closeMenu = useCallback(() => setMenuShown(null), []);

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
      const was = fit.current;
      fit.current = previewed.current ? inset(fitFrame(shrink(rect), page.current)) : fitFrame(rect, page.current);
      const now = fit.current;
      if (now.x !== was.x || now.y !== was.y || now.width !== was.width || now.height !== was.height) setFrameBox(now);
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
  function pointOf(e: { clientX: number; clientY: number }, clamp = false) {
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
  const place = (p: { x: number; y: number }) => fromPage(p, fit.current, page.current);
  const menuAction = (a: MenuAction) => {
    if (a.kind === "copy") void navigator.clipboard?.writeText(a.text).catch(() => {});
    else if (a.kind === "paste")
      void navigator.clipboard
        ?.readText()
        .then((text) => text && send({ type: "text", text }))
        .catch(() => {});
    else onMenu?.(a);
    canvas.current?.focus();
  };
  const answer = (a: BrowserUiAnswer) => {
    if (pick && !disabled) onAnswer?.(pick, a);
    canvas.current?.focus();
  };

  return (
    <div
      ref={box}
      className="bg-background absolute inset-0"
      onDragOver={(e) => {
        if (disabled) return;
        const fromPanel = !!draggedFile() && !!onDropPath && !e.dataTransfer.types.includes("Files");
        if (!fromPanel && (!onDropFiles || !e.dataTransfer.types.includes("Files"))) return;
        e.preventDefault();
        e.dataTransfer.dropEffect = "copy";
      }}
      onDrop={(e) => {
        if (disabled) return;
        const file = draggedFile();
        if (file && onDropPath && !e.dataTransfer.types.includes("Files")) {
          e.preventDefault();
          const at = pointOf(e, true);
          if (at) onDropPath(at, file);
          return;
        }
        if (!onDropFiles || e.dataTransfer.files.length === 0) return;
        e.preventDefault();
        const at = pointOf(e, true);
        if (at) onDropFiles(at, Array.from(e.dataTransfer.files));
      }}
    >
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
      {painted && tab.preview && frameBox && frameBox.width > 0 && (
        <div
          aria-hidden
          data-testid="preview-outline"
          className="ring-border pointer-events-none absolute rounded-[20px] ring-[6px]"
          style={{ left: frameBox.x, top: frameBox.y, width: frameBox.width, height: frameBox.height }}
        />
      )}
      {painted && agent && (
        <GhostCursor
          key={agent.action.tab_id}
          action={agent.action}
          seq={agent.seq}
          place={(p) => fromPage(p, fit.current, page.current)}
        />
      )}
      {painted && menu && menuShown === menu.seq && (
        <PageMenu key={menu.seq} menu={menu.menu} place={place} view={box.current} onAction={menuAction} onClose={closeMenu} />
      )}
      {painted && pick?.kind === "select" && (
        <SelectPicker key={pick.request_id} request={pick} place={place} view={box.current} onAnswer={answer} />
      )}
      {painted && pick?.kind === "date" && (
        <DatePicker key={pick.request_id} request={pick} place={place} view={box.current} onAnswer={answer} />
      )}
      {!painted && !failure && <Notice>Loading…</Notice>}
      {failure && <Notice>{failure}</Notice>}
    </div>
  );
}

/** How long the ghost cursor stays after the agent's action. */
const GHOST_MS = 1500;

/**
 * Where the agent is about to act (design 10.2): a cursor at its point and a
 * ring round the element. It moves to each new action and fades out after.
 */
export function GhostCursor({
  action,
  seq,
  place,
}: {
  action: BrowserAgentAction;
  seq: number;
  place(p: { x: number; y: number }): { x: number; y: number } | null;
}) {
  const [shown, setShown] = useState(true);
  useEffect(() => {
    setShown(true);
    const t = setTimeout(() => setShown(false), GHOST_MS);
    return () => clearTimeout(t);
  }, [seq]);
  const at = place(action);
  if (!at) return null;
  const box = action.box && place(action.box);
  const end = action.box && place({ x: action.box.x + action.box.width, y: action.box.y + action.box.height });
  return (
    <div
      aria-hidden
      data-testid="ghost-cursor"
      className={cn(
        "pointer-events-none absolute inset-0 transition-opacity duration-[130ms] ease-in motion-reduce:transition-none",
        shown ? "opacity-100" : "opacity-0",
      )}
    >
      {box && end && (
        <div
          className="ring-brand/55 absolute rounded-sm ring-2"
          style={{ left: box.x, top: box.y, width: end.x - box.x, height: end.y - box.y }}
        />
      )}
      <MousePointer2
        className="fill-brand text-background absolute size-5 drop-shadow transition-[left,top] duration-[180ms] ease-in-out motion-reduce:transition-none"
        style={{ left: at.x, top: at.y }}
      />
    </div>
  );
}
