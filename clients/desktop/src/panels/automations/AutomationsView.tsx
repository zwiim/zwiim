import { useState } from "react";
import { Button } from "@/components/ui/button";
import { errorMessage, useController } from "@/lib/host-context";
import { useAutomations, type PlaybookRow } from "./data";
import { PlaybookSection, type SectionAction } from "./PlaybookSection";
import { StartDialog } from "./StartDialog";
import { defaultOpen, runCounts } from "./status";

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
  const { state, error, rows, reload, turnOn } = useAutomations();
  const [, rerender] = useState(0);
  const [refusals, setRefusals] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState<string | null>(null);
  const [asking, setAsking] = useState<PlaybookRow | null>(null);

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
  const firstNeed = rows.find((r) => runCounts(r.items).need > 0);

  function showNeed() {
    if (!firstNeed) return;
    setOpen([firstNeed.playbook.playbook_id], true);
    document
      .querySelector(`[data-testid="playbook-${CSS.escape(firstNeed.playbook.playbook_id)}"]`)
      ?.scrollIntoView?.({ block: "start", behavior: "smooth" });
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
