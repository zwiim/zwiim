import { useEffect, useState } from "react";
import { ZwiimClientError, type ClientErrorCode, type ConnectionMode, type ConnectionPath } from "@zwiim/sdk";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { HOST_NOT_RUNNING } from "@/lib/boot";
import { errorMessage, useController, useHost } from "@/lib/host-context";

const LABELS: Record<string, string> = {
  disconnected: "Disconnected",
  connecting: "Connecting",
  handshaking: "Connecting",
  live: "Connected",
  reconnecting: "Reconnecting",
  unauthenticated: "Not authorized",
  error: "Error",
};

/** A remote host's failures, by the SDK's error code. */
const REMOTE_LABELS: Record<string, string> = {
  host_offline: "Host offline",
  cloud_unavailable: "Cloud unavailable",
  unauthenticated: "Sign-in required",
  device_refused: "Device refused",
  host_unknown: "Host unavailable",
  handshake_failed: "Handshake failed",
  lan_unavailable: "Host not reachable locally",
  host_identity_mismatch: "Host identity mismatch",
};

/** A live remote host's path. */
const PATH_LABELS: Record<ConnectionPath, string> = {
  lan: "Connected locally",
  relay: "Connected through Zwiim Cloud",
};

const MODES: { mode: ConnectionMode; label: string; detail: string }[] = [
  { mode: "auto", label: "Automatic", detail: "Direct on the same network, Zwiim Cloud everywhere else." },
  { mode: "local", label: "Local only", detail: "Only when this computer is on the host's network. Never uses the Cloud." },
  { mode: "cloud", label: "Cloud only", detail: "Always through Zwiim Cloud." },
];

/** How a remote host is reached: the user's mode, and the path taken now. */
export interface RemoteLink {
  mode: ConnectionMode;
  path: ConnectionPath | null;
  onMode(mode: ConnectionMode): void;
}

/** What a remote host's connection can ask of the user. */
export interface RemoteActions {
  /** Sign in again (the account's session lapsed). */
  onSignIn(): Promise<void>;
  /** Pair again (the host refused this device). */
  onPair(): void;
}

export function ConnectionStatus({
  onStartHost,
  remote,
  link,
}: {
  onStartHost?: () => Promise<void>;
  /** Set when the host is on another computer. */
  remote?: RemoteActions;
  link?: RemoteLink;
}) {
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
  const hostDown = !remote && conn.status === "reconnecting" && conn.error?.code === HOST_NOT_RUNNING;
  const code = conn.status !== "live" ? (conn.error?.code ?? null) : null;
  const remoteLabel = remote && code ? REMOTE_LABELS[code] : undefined;
  const signInNeeded = remote && conn.status === "unauthenticated" && code === "unauthenticated";
  const refused = remote && code === "device_refused";
  const pathLabel = conn.status === "live" && link?.path ? PATH_LABELS[link.path] : undefined;

  // An update is required: the sidebar's update card says which and offers it.
  if (conn.status === "incompatible") return null;

  return (
    <div className="flex flex-col gap-1.5" data-testid="connection-status">
      <div className="flex items-center gap-2">
        <Badge variant={variant}>
          <span className="size-1.5 rounded-full bg-current" />
          {remoteLabel ?? pathLabel ?? LABELS[conn.status] ?? conn.status}
        </Badge>
        {retryAt !== null && conn.status === "reconnecting" && (
          <span className="text-muted-foreground text-xs">
            retry in {Math.max(0, Math.ceil((retryAt - now) / 1000))}s
          </span>
        )}
      </div>
      {conn.status !== "live" && conn.error && (
        <p className="text-muted-foreground text-xs break-words">
          {remote
            ? signInNeeded
              ? "Your sign-in has lapsed. Sign in again to reach this host; it keeps running meanwhile."
              : errorMessage(new ZwiimClientError(conn.error.code as ClientErrorCode, conn.error.message))
            : conn.status === "unauthenticated"
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
        {signInNeeded && (
          <Button
            size="xs"
            variant="outline"
            onClick={async () => {
              await remote.onSignIn();
              controller.client.start();
            }}
          >
            Sign in
          </Button>
        )}
        {refused && (
          <Button size="xs" variant="outline" onClick={() => remote.onPair()}>
            Pair again
          </Button>
        )}
        {!signInNeeded && !refused && (conn.status === "unauthenticated" || conn.status === "error" || conn.status === "disconnected") && (
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

/** How a remote host is reached, one option per row with what it means. */
export function ConnectionModeChooser({ link }: { link: RemoteLink }) {
  return (
    <fieldset className="flex flex-col gap-1" aria-label="Connection mode">
      {MODES.map((m) => (
        <label
          key={m.mode}
          className="hover:bg-muted/60 has-checked:bg-muted flex cursor-pointer items-start gap-2.5 rounded-md px-2.5 py-2"
        >
          <input
            className="accent-brand mt-0.5"
            type="radio"
            name="connection-mode"
            value={m.mode}
            checked={link.mode === m.mode}
            onChange={() => link.onMode(m.mode)}
          />
          <span className="flex flex-col gap-0.5">
            <span className="text-[13px] font-medium">{m.label}</span>
            <span className="text-muted-foreground text-xs">{m.detail}</span>
          </span>
        </label>
      ))}
    </fieldset>
  );
}
