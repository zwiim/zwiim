import { useMemo, useState } from "react";
import { Loader2 } from "lucide-react";
import type { DeclaredField, RunItemView } from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import { errorMessage, useController, useHost } from "@/lib/host-context";
import { hostOf, titleOf } from "./ItemTile";
import { itemStatus } from "./status";
import { StatusDot } from "./StatusDot";

/** What a declared field will send, as the person reads it; a secret never shows. */
function fieldValue(field: DeclaredField): string {
  switch (field.value.value_kind) {
    case "text":
      return field.value.text;
    case "secret":
      return "Saved secret";
    case "input_file":
      return field.value.name;
  }
}

/**
 * The focus view's side: what the item is, and what it waits on the person
 * for. An approval offers Send or Skip; a question takes an answer.
 */
export function FocusPanel({ playbookName, item }: { playbookName: string; item: RunItemView }) {
  const controller = useController();
  const approvals = useHost((s) => s.approvals);
  const approval = useMemo(
    () => Object.values(approvals).find((a) => a.item_id === item.item_id) ?? null,
    [approvals, item.item_id],
  );
  const status = itemStatus(item);
  const site = item.site ?? hostOf(item.url) ?? "";
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [answer, setAnswer] = useState("");

  async function act(op: () => Promise<unknown>) {
    setBusy(true);
    setError(null);
    try {
      await op();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <aside className="flex w-80 shrink-0 flex-col gap-4 overflow-y-auto border-l px-5 py-4" data-testid="focus-panel">
      <div className="flex flex-col gap-1">
        <span className="text-muted-foreground truncate text-xs">
          {playbookName} · {site}
        </span>
        <h2 className="text-lg font-semibold tracking-tight">{titleOf(item)}</h2>
        <span className="text-muted-foreground flex items-center gap-1 text-xs">
          <StatusDot dot={status.dot} />
          {status.label}
        </span>
      </div>

      {approval ? (
        <div className="flex flex-col gap-3">
          <p className="text-ui">{approval.summary}</p>
          {approval.fields.length > 0 && (
            <dl className="flex flex-col gap-2">
              {approval.fields.map((field, i) => (
                <div key={i} className="flex flex-col gap-0.5">
                  <dt className="text-muted-foreground flex items-center gap-1.5 text-xs">
                    {field.target.name || field.target.role}
                    {field.prefilled && (
                      <span className="bg-secondary text-secondary-foreground rounded-[6px] px-1.5 text-2xs">
                        Pre-filled
                      </span>
                    )}
                  </dt>
                  <dd className="text-ui break-words">{fieldValue(field)}</dd>
                </div>
              ))}
            </dl>
          )}
          <div className="flex justify-end gap-2">
            <Button
              variant="secondary"
              size="sm"
              disabled={busy}
              onClick={() => void act(() => controller.reject(approval.approval_id))}
            >
              Skip
            </Button>
            <Button
              variant="brand"
              size="sm"
              disabled={busy}
              onClick={() => void act(() => controller.approve(approval.approval_id))}
            >
              {busy && <Loader2 className="animate-spin" />}
              Send
            </Button>
          </div>
        </div>
      ) : item.status === "waiting_person" ? (
        <form
          className="flex flex-col gap-3"
          onSubmit={(e) => {
            e.preventDefault();
            const text = answer.trim();
            if (text) void act(() => controller.answer(item.item_id, text));
          }}
        >
          <p className="text-ui">{item.question ?? "The page needs an answer only you have."}</p>
          <input
            className="border-border bg-input/30 text-ui focus-visible:ring-ring rounded-[10px] border px-2.5 py-1.5 outline-none focus-visible:ring-2"
            aria-label="Your answer"
            value={answer}
            disabled={busy}
            onChange={(e) => setAnswer(e.target.value)}
          />
          <div className="flex justify-end">
            <Button type="submit" variant="brand" size="sm" disabled={busy || answer.trim() === ""}>
              {busy && <Loader2 className="animate-spin" />}
              Save and retry
            </Button>
          </div>
        </form>
      ) : (
        <p className="text-muted-foreground text-ui">
          {item.status === "working"
            ? "Working on it. The page is live."
            : item.status === "queued"
              ? "Waiting its turn."
              : status.label}
        </p>
      )}

      {error && (
        <p className="text-destructive text-xs" role="alert">
          {error}
        </p>
      )}
    </aside>
  );
}
