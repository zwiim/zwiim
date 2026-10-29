// The follow-ups waiting for the agent's running turn to end, above the
// composer. The host keeps the queue, so every device shows and changes the
// same one; it sends the first when a turn ends normally and holds the queue
// after a stopped or failed turn until the user sends one.

import { useState } from "react";
import { ArrowUp, Paperclip, Pencil, X } from "lucide-react";
import type { QueuedFollowUp } from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import { errorMessage, useController } from "@/lib/host-context";

export function FollowUpQueue({
  sessionId,
  items,
  running,
  disabled,
}: {
  sessionId: string;
  items: readonly QueuedFollowUp[];
  /** The agent is in a turn: the first goes out when it ends. */
  running: boolean;
  disabled: boolean;
}) {
  const controller = useController();
  const [editing, setEditing] = useState<{ id: string; text: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function act(run: () => Promise<unknown>) {
    setBusy(true);
    setError(null);
    try {
      await run();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  function save() {
    if (!editing) return;
    const { id, text } = editing;
    void act(async () => {
      await controller.editFollowUp(sessionId, id, text.trim());
      setEditing(null);
    });
  }

  return (
    <section className="bg-card animate-enter flex flex-col gap-1 rounded-lg px-3 py-2 text-xs shadow-sm" data-testid="follow-ups">
      <span className="text-muted-foreground" role="status">
        {running ? "Queued" : "Paused · Send one to carry on"}
      </span>
      {items.map((f, i) => (
        <div key={f.follow_up_id} className="flex min-w-0 items-center gap-2" data-testid="follow-up">
          {editing?.id === f.follow_up_id ? (
            <input
              autoFocus
              aria-label="Follow-up text"
              className="bg-background ring-border focus:ring-ring/60 min-w-0 flex-1 rounded-md px-2 py-1 ring-1 outline-none"
              value={editing.text}
              disabled={busy}
              onChange={(e) => setEditing({ id: f.follow_up_id, text: e.target.value })}
              onKeyDown={(e) => {
                if (e.key === "Enter" && !e.nativeEvent.isComposing) {
                  e.preventDefault();
                  save();
                } else if (e.key === "Escape") {
                  e.preventDefault();
                  setEditing(null);
                }
              }}
              onBlur={() => setEditing(null)}
            />
          ) : (
            <span className="min-w-0 flex-1 truncate" title={f.text}>
              {f.text || <span className="text-muted-foreground">Attachments only</span>}
            </span>
          )}
          {f.attachments > 0 && (
            <span className="text-muted-foreground flex shrink-0 items-center gap-0.5" title="Attached files">
              <Paperclip className="size-3" />
              {f.attachments}
            </span>
          )}
          <Button
            size="xs"
            variant="ghost"
            className="w-6 px-0"
            aria-label="Edit follow-up"
            title="Edit"
            disabled={disabled || busy}
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => (editing?.id === f.follow_up_id ? save() : setEditing({ id: f.follow_up_id, text: f.text }))}
          >
            <Pencil />
          </Button>
          <Button
            size="xs"
            variant="ghost"
            className="w-6 px-0"
            aria-label="Remove follow-up"
            title="Remove"
            disabled={disabled || busy}
            onClick={() => void act(() => controller.removeFollowUp(sessionId, f.follow_up_id))}
          >
            <X />
          </Button>
          {/* While a turn runs the first already goes next. */}
          {(!running || i > 0) && (
            <Button
              size="xs"
              variant="outline"
              title={running ? "Send this one when the turn ends" : "Send this one now"}
              disabled={disabled || busy}
              onClick={() => void act(() => controller.sendQueuedFollowUp(sessionId, f.follow_up_id))}
            >
              <ArrowUp />
              {running ? "Send next" : "Send now"}
            </Button>
          )}
        </div>
      ))}
      {error && (
        <p className="text-destructive" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}
