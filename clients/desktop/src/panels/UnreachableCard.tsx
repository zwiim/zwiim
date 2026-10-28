import { useState } from "react";
import { Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { errorMessage, useController } from "@/lib/host-context";

/**
 * The agent could not reach its provider. The Host kept the failed turn, so the
 * user can send it again here or carry it into a new chat. Nothing runs until a
 * button is tapped; an op refused (or left unknown) shows inline and frees the
 * buttons, while one that went through keeps them off until the card goes.
 */
export function UnreachableCard({
  agentName,
  message,
  sessionId,
  online,
  onStarted,
}: {
  agentName: string;
  message: string;
  sessionId: string;
  /** Offline: the card stays visible but cannot be used. */
  online: boolean;
  /** The new session a "Retry in new chat" started. */
  onStarted(id: string): void;
}) {
  const controller = useController();
  /** The button whose op is in flight: it shows a spinner. */
  const [pending, setPending] = useState<"retry" | "new_chat" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const busy = pending !== null || !online;

  async function run(which: "retry" | "new_chat") {
    setPending(which);
    setError(null);
    let started: string | null = null;
    try {
      if (which === "retry") {
        await controller.retrySession(sessionId);
      } else {
        started = (await controller.client.request("session.retry_in_new_chat", { session_id: sessionId })).session_id;
      }
    } catch (e) {
      setError(errorMessage(e));
      setPending(null);
    }
    // On success `pending` stays set: the card is still up until the new turn
    // clears the error, and a second tap there would only be refused.
    // Outside the try: a throwing callback is not the Host refusing.
    if (started) onStarted(started);
  }

  return (
    <div
      // `surface` card with the 3pt `err` rule on its leading edge.
      className="bg-card animate-enter flex flex-col gap-2 rounded-xl p-4 shadow-[inset_3px_0_0_var(--destructive)] motion-reduce:animate-none"
      data-testid="unreachable-card"
    >
      <p className="text-sm">
        {agentName} error: {message}
      </p>
      <div className="flex flex-wrap gap-2">
        <Button
          size="sm"
          variant="ghost"
          className="text-muted-foreground"
          disabled={busy}
          onClick={() => void run("retry")}
        >
          {pending === "retry" && <Loader2 className="animate-spin" />}
          Retry →
        </Button>
        <Button
          size="sm"
          variant="ghost"
          className="text-muted-foreground"
          disabled={busy}
          onClick={() => void run("new_chat")}
        >
          {pending === "new_chat" && <Loader2 className="animate-spin" />}
          Retry in new chat →
        </Button>
      </div>
      {error && (
        <p className="text-destructive text-xs" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}
