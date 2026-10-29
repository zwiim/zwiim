// Finding a tab or setting by typing: every tab and setting this window offers,
// matched loosely on what it shows (title, description) and on its keywords.

import type { ReactNode } from "react";
import fuzzysort from "fuzzysort";
import { cn } from "@/lib/utils";
import {
  SETTINGS,
  TABS,
  visibleSettings,
  visibleTabs,
  type SettingDef,
  type SettingsContext,
  type TabDef,
} from "@/settings/registry";
import type { SettingsTarget } from "@/settings/open";

/** Below this a match is too loose to show (1 is exact). */
const THRESHOLD = 0.5;
const LIMIT = 30;

interface Entry {
  tab: TabDef;
  setting?: SettingDef;
  title: string;
  description: string;
  keywords: string;
}

export interface SearchHit {
  target: SettingsTarget;
  /** The title with what matched marked. */
  title: ReactNode[];
  /** The tab it is in; absent for a tab itself. */
  tabLabel?: string;
}

/** The tabs and settings `query` finds, best first. */
export function searchSettings(
  query: string,
  ctx: SettingsContext,
  tabs: readonly TabDef[] = TABS,
  settings: readonly SettingDef[] = SETTINGS,
): SearchHit[] {
  const q = query.trim();
  if (!q) return [];
  const entries: Entry[] = [];
  for (const tab of visibleTabs(ctx, tabs)) {
    entries.push({ tab, title: tab.label, description: "", keywords: (tab.keywords ?? []).join(" ") });
    for (const setting of visibleSettings(tab.id, ctx, settings)) {
      entries.push({
        tab,
        setting,
        title: setting.title,
        description: setting.description ?? "",
        keywords: (setting.keywords ?? []).join(" "),
      });
    }
  }
  const results = fuzzysort.go(q, entries, {
    keys: ["title", "description", "keywords"],
    threshold: THRESHOLD,
    limit: LIMIT,
  });
  return results.map((r) => {
    const { tab, setting, title } = r.obj;
    const marked = r[0]!.indexes.length > 0 ? r[0]!.highlight((m, i) => <mark key={i}>{m}</mark>) : [title];
    return setting
      ? { target: { tab: tab.id, settingId: setting.id }, title: marked, tabLabel: tab.label }
      : { target: { tab: tab.id }, title: marked };
  });
}

/** The hits, one picked by the arrow keys; a click or Enter opens it. */
export function SearchResults({
  hits,
  active,
  onActive,
  onPick,
}: {
  hits: SearchHit[];
  active: number;
  onActive(index: number): void;
  onPick(target: SettingsTarget): void;
}) {
  if (hits.length === 0) return <p className="text-muted-foreground">No settings match.</p>;
  return (
    <ul className="flex flex-col gap-0.5" role="listbox" id="settings-search-results" aria-label="Search results">
      {hits.map((hit, i) => (
        <li
          key={`${hit.target.tab}/${hit.target.settingId ?? ""}`}
          id={`settings-search-${i}`}
          className={cn(
            "text-ui flex cursor-default items-baseline gap-2 rounded-md px-2.5 py-1.5",
            "[&_mark]:text-foreground [&_mark]:bg-transparent [&_mark]:font-semibold",
            i === active && "bg-muted",
          )}
          role="option"
          aria-selected={i === active}
          onMouseEnter={() => onActive(i)}
          onMouseDown={(e) => {
            // Keeps focus in the search field.
            e.preventDefault();
            onPick(hit.target);
          }}
        >
          <span className="min-w-0 flex-1 truncate">{hit.title}</span>
          {hit.tabLabel && <span className="text-muted-foreground shrink-0 text-xs">{hit.tabLabel}</span>}
        </li>
      ))}
    </ul>
  );
}
