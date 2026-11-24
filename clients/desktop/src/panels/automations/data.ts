import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  isClientError,
  type PlaybookSummary,
  type RunItemView,
  type RunOutcome,
  type RunStatus,
  type SiteSummary,
} from "@zwiim/sdk";
import { errorMessage, useController, useHost } from "@/lib/host-context";

/** A playbook, its latest run and that run's items. */
export interface PlaybookRow {
  playbook: PlaybookSummary;
  run: {
    run_id: string;
    status: RunStatus;
    outcome?: RunOutcome | null;
    error?: string | null;
  } | null;
  items: RunItemView[];
  /** The run's items are in the store; false until they load (again after a new snapshot). */
  itemsLoaded: boolean;
}

export interface Automations {
  state: "loading" | "off" | "ready" | "error";
  error: string | null;
  rows: PlaybookRow[];
  /** The playbooks' sites that are paused, each waiting on the person. */
  pausedSites: SiteSummary[];
  reload(): void;
  turnOn(): Promise<void>;
}

interface LatestRun {
  run_id: string;
  playbook_id: string;
  status: RunStatus;
  outcome?: RunOutcome | null;
  error?: string | null;
  created_at: string;
}

interface Listed {
  playbooks: PlaybookSummary[];
  runs: LatestRun[];
}

/** A run's items come in pages; stop after this many. */
const MAX_ITEM_PAGES = 10;

/**
 * The playbooks, each with its latest run and that run's items. The lists
 * load once; the Host feed keeps runs and items live after that.
 */
export function useAutomations(): Automations {
  const controller = useController();
  const liveRuns = useHost((s) => s.runs);
  const runItems = useHost((s) => s.runItems);
  const [listed, setListed] = useState<Listed | null>(null);
  const [state, setState] = useState<Automations["state"]>("loading");
  const [error, setError] = useState<string | null>(null);
  const [generation, setGeneration] = useState(0);
  const reload = useCallback(() => setGeneration((g) => g + 1), []);

  useEffect(() => {
    let live = true;
    (async () => {
      try {
        const playbooks = await controller.listPlaybooks();
        const page = await controller.listRuns({ limit: 100 });
        if (!live) return;
        setListed({ playbooks, runs: page.runs });
        setState("ready");
        setError(null);
      } catch (e) {
        if (!live) return;
        if (isClientError(e, "playbooks_disabled")) {
          setState("off");
        } else {
          setState("error");
          setError(errorMessage(e));
        }
      }
    })();
    return () => {
      live = false;
    };
  }, [controller, generation]);

  // Sites pause from inside a run (a sign-in page) with no feed event of their own:
  // read them again whenever the items move.
  const [sites, setSites] = useState<SiteSummary[]>([]);
  useEffect(() => {
    let live = true;
    controller.listSites().then(
      (list) => live && setSites(list),
      () => {},
    );
    return () => {
      live = false;
    };
  }, [controller, generation, runItems]);

  const turnOn = useCallback(async () => {
    await controller.client.request("settings.update", {
      values: { "playbooks.enabled": true },
    });
    reload();
  }, [controller, reload]);

  const latest = useMemo(() => {
    const byPlaybook = new Map<string, LatestRun>();
    // The feed's copy wins over the listed one for the same run: it is live.
    const runs = new Map<string, LatestRun>();
    for (const r of listed?.runs ?? []) runs.set(r.run_id, r);
    for (const r of Object.values(liveRuns)) runs.set(r.run_id, r);
    for (const r of runs.values()) {
      const prev = byPlaybook.get(r.playbook_id);
      if (!prev || r.created_at > prev.created_at)
        byPlaybook.set(r.playbook_id, r);
    }
    return byPlaybook;
  }, [listed, liveRuns]);

  // Only a new snapshot swaps the items for an empty set. Runs that ended while
  // the feed was away left it unseen, and only `run.list` has their final state.
  const lastItems = useRef(runItems);
  useEffect(() => {
    if (runItems === lastItems.current) return;
    lastItems.current = runItems;
    if (Object.keys(runItems).length === 0) reload();
  }, [runItems, reload]);

  // A run of a playbook the list does not have yet was started elsewhere: list again, once per run.
  const seenUnknown = useRef(new Set<string>());
  useEffect(() => {
    if (!listed) return;
    const known = new Set(listed.playbooks.map((p) => p.playbook_id));
    for (const r of Object.values(liveRuns)) {
      if (known.has(r.playbook_id) || seenUnknown.current.has(r.run_id))
        continue;
      seenUnknown.current.add(r.run_id);
      reload();
      return;
    }
  }, [listed, liveRuns, reload]);

  // Load the items of each latest run the store lacks (first view, or a new snapshot emptied them).
  const loaded = useRef(new Set<string>());
  const loading = useRef(new Set<string>());
  useEffect(() => {
    if (!listed) return;
    for (const r of latest.values()) {
      const id = r.run_id;
      if (id in runItems && loaded.current.has(id)) continue;
      if (loading.current.has(id)) continue;
      loading.current.add(id);
      loaded.current.delete(id);
      (async () => {
        try {
          let cursor: string | null | undefined;
          for (let page = 0; page < MAX_ITEM_PAGES; page++) {
            cursor = await controller.loadRunItems(id, cursor ?? undefined);
            if (cursor === null) break;
          }
          loaded.current.add(id);
        } catch {
          // The section shows the run without items; the next change or reload tries again.
        } finally {
          loading.current.delete(id);
        }
      })();
    }
  }, [controller, listed, latest, runItems]);

  const rows = useMemo<PlaybookRow[]>(
    () =>
      (listed?.playbooks ?? []).map((playbook) => {
        const r = latest.get(playbook.playbook_id);
        return {
          playbook,
          run: r
            ? {
                run_id: r.run_id,
                status: r.status,
                outcome: r.outcome ?? null,
                error: r.error ?? null,
              }
            : null,
          items: r ? Object.values(runItems[r.run_id] ?? {}) : [],
          itemsLoaded: !r || r.run_id in runItems,
        };
      }),
    [listed, latest, runItems],
  );

  const pausedSites = useMemo(() => {
    const ours = new Set(
      (listed?.playbooks ?? []).flatMap((p) =>
        (p.sites ?? []).map((s) => s.site),
      ),
    );
    return sites.filter((s) => s.paused_cause && ours.has(s.site));
  }, [listed, sites]);

  return { state, error, rows, pausedSites, reload, turnOn };
}
