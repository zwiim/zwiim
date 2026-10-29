import { Fragment, useEffect, useRef, useState, type RefObject } from "react";
import { Search, X } from "lucide-react";
import { hasCapability } from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import { Dialog } from "@/components/ui/dialog";
import type { AccountState } from "@/lib/account";
import type { NativeBridge } from "@/lib/bridge";
import { useHost } from "@/lib/host-context";
import { formatShortcut, isModKey } from "@/lib/platform";
import { cn } from "@/lib/utils";
import type { RemoteLink } from "@/panels/ConnectionStatus";
import { useGithubSignInOffered } from "@/panels/Github";
import { useInstructionsOffered } from "@/panels/Instructions";
import type { SettingsTarget } from "@/settings/open";
import { GROUPS, visibleSettings, visibleTabs, type SettingsContext } from "@/settings/registry";
import { SearchResults, searchSettings } from "@/settings/search";

export type { SettingsTab } from "@/settings/registry";

/** How long a setting opened by a link or a search stays marked. */
const FLASH_MS = 1500;
/** How long to wait for that setting to appear (some read their state first). */
const FIND_MS = 3000;

/**
 * Everything about this app and the host it shows, one pane per tab. What
 * belongs to this computer's host only (its devices, starting at login) is
 * not offered for a remote host; how a remote host is reached is offered for
 * it alone. Which tabs and settings exist is the registry's to say.
 * Typing in the search field (focused by Mod+F) lists matching tabs and
 * settings across every tab; picking one opens it like a link would.
 */
export function SettingsDialog({
  bridge,
  remote,
  account,
  link,
  updateHint,
  workspaceId,
  target,
  onTarget,
  onClose,
}: {
  bridge: NativeBridge;
  remote: boolean;
  account: AccountState;
  link?: RemoteLink;
  updateHint: boolean;
  /** What the window shows. */
  workspaceId?: string | null;
  target: SettingsTarget;
  onTarget(target: SettingsTarget): void;
  onClose(): void;
}) {
  const githubOffered = useGithubSignInOffered();
  const instructions = useInstructionsOffered();
  const hostSettings = useHost((s) => hasCapability(s.connection.compat, "settings"));
  const gitSettings = useHost((s) => hasCapability(s.connection.compat, "settings.git"));
  const promptSettings = useHost((s) => hasCapability(s.connection.compat, "settings.prompts"));
  const repositorySettings = useHost((s) => hasCapability(s.connection.compat, "repository.settings"));
  const advancedSettings = useHost((s) => hasCapability(s.connection.compat, "settings.advanced"));
  const steer = useHost((s) => hasCapability(s.connection.compat, "session.steer"));
  const attachments = useHost((s) => hasCapability(s.connection.compat, "attachments"));
  const autoMerge = useHost((s) => hasCapability(s.connection.compat, "git.pr_auto_merge"));
  const ctx: SettingsContext = {
    bridge,
    remote,
    account,
    link,
    updateHint,
    githubOffered,
    hostSettings,
    instructions,
    gitSettings,
    promptSettings,
    repositorySettings,
    advancedSettings,
    steer,
    attachments,
    autoMerge,
    workspaceId: target.workspaceId ?? workspaceId,
  };
  const tabs = visibleTabs(ctx);
  const current = tabs.find((t) => t.id === target.tab) ?? tabs[0]!;
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const searchField = useRef<HTMLInputElement>(null);
  const body = useRef<HTMLDivElement>(null);
  const searching = query.trim() !== "";
  const hits = searchSettings(query, ctx);

  useFlash(body, target);

  const pick = (to: SettingsTarget) => {
    setQuery("");
    // Settings opened for one repository stay on it from tab to tab.
    onTarget({ workspaceId: target.workspaceId, ...to });
  };

  return (
    <Dialog
      className="h-[min(640px,100%)] max-w-3xl flex-row overflow-hidden"
      aria-label="Settings"
      onClose={onClose}
      onKeyDown={(e) => {
        if (isModKey(e) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "f") {
          e.preventDefault();
          searchField.current?.focus();
          searchField.current?.select();
        }
      }}
    >
      <nav className="bg-sidebar flex w-52 shrink-0 flex-col gap-0.5 p-2" aria-label="Settings sections">
        <label className="bg-background/60 ring-border focus-within:ring-ring mb-1 flex items-center gap-2 rounded-md px-2 py-1 ring-1 transition-shadow">
          <Search className="text-muted-foreground size-3.5 shrink-0" />
          <input
            ref={searchField}
            aria-label="Search settings"
            aria-controls={searching ? "settings-search-results" : undefined}
            aria-activedescendant={searching && hits.length > 0 ? `settings-search-${active}` : undefined}
            placeholder="Search"
            className="text-ui min-w-0 flex-1 bg-transparent outline-none"
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setActive(0);
            }}
            onKeyDown={(e) => {
              if (e.key === "ArrowDown" || e.key === "ArrowUp") {
                e.preventDefault();
                if (hits.length === 0) return;
                const step = e.key === "ArrowDown" ? 1 : -1;
                setActive((i) => (i + step + hits.length) % hits.length);
              } else if (e.key === "Enter" && !e.nativeEvent.isComposing) {
                const hit = hits[active];
                if (hit) pick(hit.target);
              } else if (e.key === "Escape" && query) {
                // Clears the search; a second Escape closes settings.
                e.stopPropagation();
                setQuery("");
              }
            }}
          />
          {!query && <kbd className="text-muted-foreground font-sans text-2xs">{formatShortcut("Mod+F")}</kbd>}
        </label>
        {GROUPS.map((group) => {
          const inGroup = tabs.filter((t) => t.group === group);
          if (inGroup.length === 0) return null;
          return (
            <div key={group} className="flex flex-col gap-0.5">
              <span className="text-muted-foreground px-2 pt-3 pb-1 text-2xs font-semibold tracking-wide uppercase">
                {group}
              </span>
              {inGroup.map((t) => {
                const hint = t.hint?.(ctx);
                return (
                  <button
                    key={t.id}
                    className={cn(
                      "flex items-center gap-2 rounded-md px-2 py-1.5 text-left text-ui transition-colors",
                      t.id === current.id
                        ? "bg-sidebar-accent text-foreground"
                        : "text-muted-foreground hover:text-foreground hover:bg-sidebar-accent/60",
                    )}
                    aria-current={t.id === current.id ? "page" : undefined}
                    onClick={() => pick({ tab: t.id })}
                  >
                    <t.icon className="size-4 shrink-0" />
                    <span className="flex-1">{t.label}</span>
                    {hint && <span className="bg-brand size-2 rounded-full" aria-label={hint} />}
                  </button>
                );
              })}
            </div>
          );
        })}
      </nav>
      <div className="relative flex min-w-0 flex-1 flex-col">
        <Button className="absolute top-3 right-3" size="icon-sm" variant="ghost" title="Close" onClick={onClose}>
          <X />
        </Button>
        {searching ? (
          <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto px-7 py-6 text-xs" data-testid="settings-search">
            <h2 className="mb-1 text-lg font-semibold tracking-tight">Search</h2>
            <SearchResults hits={hits} active={active} onActive={setActive} onPick={pick} />
          </div>
        ) : (
          <div
            ref={body}
            className={cn(
              "flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto px-7 py-6 text-xs",
              // A setting opened by a link or a search is ringed for a moment.
              "[&_[data-flash]]:ring-brand [&_[data-flash]]:rounded-md [&_[data-flash]]:ring-2",
            )}
            data-testid={`settings-${current.id}`}
          >
            <h2 className="mb-1 text-lg font-semibold tracking-tight">{current.label}</h2>
            {current.panel?.(ctx)}
            {visibleSettings(current.id, ctx).map((s) => (
              <Fragment key={s.id}>{s.render(ctx, s)}</Fragment>
            ))}
          </div>
        )}
      </div>
    </Dialog>
  );
}

/**
 * Scrolls to the setting `target` names and marks it for a moment. A row that
 * reads its state before showing is waited for, briefly.
 */
function useFlash(body: RefObject<HTMLDivElement | null>, target: SettingsTarget) {
  useEffect(() => {
    const root = body.current;
    const id = target.settingId;
    if (!root || !id) return;
    let marked: HTMLElement | null = null;
    let unmark: ReturnType<typeof setTimeout> | undefined;
    const find = () => {
      const row = root.querySelector<HTMLElement>(`[data-setting-id="${id}"]`);
      if (!row) return false;
      marked = row;
      const reduce = window.matchMedia?.("(prefers-reduced-motion: reduce)")?.matches ?? true;
      row.scrollIntoView?.({ block: "center", behavior: reduce ? "auto" : "smooth" });
      row.setAttribute("data-flash", "");
      unmark = setTimeout(() => row.removeAttribute("data-flash"), FLASH_MS);
      return true;
    };
    const watch = new MutationObserver(() => {
      if (find()) watch.disconnect();
    });
    if (!find()) watch.observe(root, { childList: true, subtree: true });
    const giveUp = setTimeout(() => watch.disconnect(), FIND_MS);
    return () => {
      watch.disconnect();
      clearTimeout(giveUp);
      clearTimeout(unmark);
      marked?.removeAttribute("data-flash");
    };
  }, [body, target]);
}
