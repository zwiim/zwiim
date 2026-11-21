import { useCallback, useState } from "react";
import { Button } from "@/components/ui/button";
import { errorMessage, useController, useHost } from "@/lib/host-context";
import { useAutomations, type PlaybookRow } from "./data";
import { FocusView } from "./FocusView";
import { PlaybookSection, type SectionAction } from "./PlaybookSection";
import { StartDialog } from "./StartDialog";
import { defaultOpen, pausedText, runCounts } from "./status";

/** The person's open/closed choice per playbook; it wins over the default for the session. */
const toggles = new Map<string, boolean>();

/** Forget the person's choices (tests). */
export function resetAutomationsView() {
  toggles.clear();
}

function isOpen(row: PlaybookRow): boolean {
  return toggles.get(row.playbook.playbook_id) ?? defaultOpen(row.run, runCounts(row.items));
}

/** The Automations canvas: each playbook a section with its latest run's items. */
export function AutomationsView() {
  const controller = useController();
  const { state, error, rows, pausedSites, reload, turnOn } = useAutomations();
  const [, rerender] = useState(0);
  const [refusals, setRefusals] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState<string | null>(null);
  const [asking, setAsking] = useState<PlaybookRow | null>(null);
  const [siteError, setSiteError] = useState<string | null>(null);
  /** The item shown large, if any. */
  const [focus, setFocus] = useState<{ playbook_id: string; item_id: string } | null>(null);
  const closeFocus = useCallback(() => setFocus(null), []);
  const approvals = useHost((s) => s.approvals);

  function setOpen(ids: string[], open: boolean) {
    for (const id of ids) toggles.set(id, open);
    rerender((n) => n + 1);
  }

  async function act(row: PlaybookRow, op: () => Promise<unknown>) {
    const id = row.playbook.playbook_id;
    setBusy(id);
    setRefusals(({ [id]: _, ...rest }) => rest);
    try {
      await op();
    } catch (e) {
      setRefusals((r) => ({ ...r, [id]: errorMessage(e) }));
    } finally {
      setBusy(null);
      reload();
    }
  }

  function onAction(row: PlaybookRow, action: SectionAction) {
    const id = row.playbook.playbook_id;
    if (action === "pause") return void act(row, () => controller.pauseRun(row.run!.run_id));
    if (action === "resume") return void act(row, () => controller.resumeRun(row.run!.run_id));
    if ((row.playbook.sites ?? []).some((s) => !s.risk_accepted)) return setAsking(row);
    void act(row, () => controller.startRun(id));
  }

  async function resumeSite(site: string) {
    setSiteError(null);
    try {
      await controller.resumeSite(site);
    } catch (e) {
      setSiteError(errorMessage(e));
    } finally {
      reload();
    }
  }

  async function startAccepting(row: PlaybookRow) {
    await act(row, async () => {
      for (const s of row.playbook.sites ?? []) if (!s.risk_accepted) await controller.acceptSiteRisk(s.site);
      await controller.startRun(row.playbook.playbook_id);
    });
  }

  if (state === "loading") return <p className="text-muted-foreground m-auto text-sm">Loading…</p>;
  if (state === "off") {
    return (
      <div className="m-auto flex flex-col items-center gap-2 text-center">
        <p className="text-lg font-semibold tracking-tight">Automations are off</p>
        <Button variant="brand" size="sm" onClick={() => void turnOn().catch(() => {})}>
          Turn them on
        </Button>
      </div>
    );
  }
  if (state === "error") return <p className="text-destructive m-auto text-sm">{error}</p>;

  const running = rows.filter((r) => r.run?.status === "running" || r.run?.status === "discovering").length;
  const need = rows.reduce((n, r) => n + runCounts(r.items).need, 0);
  const allOpen = rows.length > 0 && rows.every(isOpen);

  /** Open the item that has waited longest: the oldest approval's, else the first question. */
  function showNeed() {
    const waiting = Object.values(approvals).sort((a, b) => a.created_at.localeCompare(b.created_at));
    for (const a of waiting) {
      const row = rows.find((r) => r.items.some((i) => i.item_id === a.item_id));
      if (row) return setFocus({ playbook_id: row.playbook.playbook_id, item_id: a.item_id });
    }
    for (const row of rows) {
      const asked = row.items.find((i) => i.status === "waiting_person" || i.status === "waiting_approval");
      if (asked) return setFocus({ playbook_id: row.playbook.playbook_id, item_id: asked.item_id });
    }
  }

  const focusRow = focus ? rows.find((r) => r.playbook.playbook_id === focus.playbook_id) : undefined;
  if (focus && focusRow) {
    return (
      <FocusView
        row={focusRow}
        itemId={focus.item_id}
        onMove={(item_id) => setFocus({ playbook_id: focusRow.playbook.playbook_id, item_id })}
        onClose={closeFocus}
      />
    );
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-y-auto px-6 py-4">
      <header className="flex items-center gap-3 pb-2" data-testid="automations-header">
        <h1 className="text-lg font-semibold tracking-tight">Automations</h1>
        <span className="text-muted-foreground font-mono text-xs tabular-nums">
          {running} running · {rows.length} playbooks
        </span>
        {need > 0 && (
          <button
            className="bg-brand-soft text-brand rounded-full px-2 py-0.5 font-mono text-xs tabular-nums"
            onClick={showNeed}
          >
            {need} need you
          </button>
        )}
        <span className="flex-1" />
        {rows.length > 0 && (
          <Button
            variant="ghost"
            size="xs"
            onClick={() =>
              setOpen(
                rows.map((r) => r.playbook.playbook_id),
                !allOpen,
              )
            }
          >
            {allOpen ? "Collapse all" : "Expand all"}
          </Button>
        )}
      </header>
      {pausedSites.length > 0 && (
        <div className="flex flex-col gap-2 pb-2" data-testid="paused-sites">
          {pausedSites.map((s) => (
            <div key={`${s.site} ${s.account}`} className="bg-card flex items-center gap-3 rounded-[10px] border px-3 py-2">
              <p className="text-ui flex-1">
                <span className="font-medium">{s.site} is paused.</span> {pausedText(s.paused_cause)}
              </p>
              <Button variant="secondary" size="sm" onClick={() => void resumeSite(s.site)}>
                Resume
              </Button>
            </div>
          ))}
          {siteError && (
            <p className="text-destructive text-xs" role="alert">
              {siteError}
            </p>
          )}
        </div>
      )}
      {rows.length === 0 ? (
        <p className="text-muted-foreground m-auto text-sm">No playbooks yet. Ask an agent in a chat to write one.</p>
      ) : (
        rows.map((row) => {
          const id = row.playbook.playbook_id;
          const open = isOpen(row);
          return (
            <PlaybookSection
              key={id}
              row={row}
              open={open}
              error={refusals[id] ?? null}
              busy={busy === id}
              onToggle={() => setOpen([id], !open)}
              onAction={(action) => onAction(row, action)}
              onOpen={(item_id) => setFocus({ playbook_id: id, item_id })}
            />
          );
        })
      )}
      {asking && (
        <StartDialog
          name={asking.playbook.name}
          sites={(asking.playbook.sites ?? []).filter((s) => !s.risk_accepted).map((s) => s.site)}
          onStart={() => startAccepting(asking)}
          onClose={() => setAsking(null)}
        />
      )}
    </div>
  );
}
