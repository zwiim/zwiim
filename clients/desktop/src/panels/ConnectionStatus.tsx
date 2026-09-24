import { useEffect, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { HOST_NOT_RUNNING } from "@/lib/boot";
import { useController, useHost } from "@/lib/host-context";

const LABELS: Record<string, string> = {
  disconnected: "Disconnected",
  connecting: "Connecting",
  handshaking: "Connecting",
  live: "Connected",
  reconnecting: "Reconnecting",
  unauthenticated: "Not authorized",
  error: "Error",
};

export function ConnectionStatus({ onStartHost }: { onStartHost?: () => Promise<void> }) {
  const controller = useController();
  const conn = useHost((s) => s.connection);
  const [starting, setStarting] = useState(false);
  const [startError, setStartError] = useState<string | null>(null);
  const [now, setNow] = useState(Date.now());
  const [retryAt, setRetryAt] = useState<number | null>(null);

  useEffect(() => {
    setRetryAt(conn.retryInMs !== null ? Date.now() + conn.retryInMs : null);
  }, [conn]);
  useEffect(() => {
    if (retryAt === null) return;
    const t = setInterval(() => setNow(Date.now()), 500);
    return () => clearInterval(t);
  }, [retryAt]);

  const variant =
    conn.status === "live"
      ? "success"
      : conn.status === "unauthenticated" || conn.status === "error"
        ? "destructive"
        : "warning";
  const hostDown = conn.status === "reconnecting" && conn.error?.code === HOST_NOT_RUNNING;

  return (
    <div className="flex flex-col gap-1.5" data-testid="connection-status">
      <div className="flex items-center gap-2">
        <Badge variant={variant}>
          <span className="size-1.5 rounded-full bg-current" />
          {LABELS[conn.status] ?? conn.status}
        </Badge>
        {retryAt !== null && conn.status === "reconnecting" && (
          <span className="text-muted-foreground text-xs">
            retry in {Math.max(0, Math.ceil((retryAt - now) / 1000))}s
          </span>
        )}
      </div>
      {conn.status !== "live" && conn.error && (
        <p className="text-muted-foreground text-xs break-words">
          {conn.status === "unauthenticated"
            ? "The host no longer accepts this app's credentials (it may have restarted). Reconnect to pick up the current ones."
            : hostDown
              ? `${conn.error.message}. Your workspaces and sessions are kept; start the host to continue.`
              : conn.error.message}
        </p>
      )}
      {startError && (
        <p className="text-destructive text-xs break-words" role="alert">
          Could not start the host: {startError}
        </p>
      )}
      <div className="flex gap-1.5">
        {conn.status === "reconnecting" && (
          <Button size="xs" variant="outline" onClick={() => controller.client.retryNow()}>
            Retry now
          </Button>
        )}
        {(conn.status === "unauthenticated" || conn.status === "error" || conn.status === "disconnected") && (
          <Button size="xs" variant="outline" onClick={() => controller.client.start()}>
            Reconnect
          </Button>
        )}
        {hostDown && onStartHost && (
          <Button
            size="xs"
            variant="outline"
            disabled={starting}
            onClick={async () => {
              setStarting(true);
              setStartError(null);
              try {
                await onStartHost();
                controller.client.retryNow();
              } catch (e) {
                setStartError(e instanceof Error ? e.message : String(e));
              } finally {
                setStarting(false);
              }
            }}
          >
            {starting ? "Starting…" : "Start host"}
          </Button>
        )}
      </div>
    </div>
  );
}
