import { useCallback, useEffect, useState } from "react";
import { Loader2, RefreshCw } from "lucide-react";
import type { AgentAuthStatus } from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import { errorMessage, useController, useOnline } from "@/lib/host-context";
import { cn } from "@/lib/utils";

/** While a sign-in page is open, how often to look for the result. */
const POLL_MS = 2000;

type Agent = "claude_code" | "codex";

const AGENTS: { id: Agent; label: string; account: string; keyName: string; keyHint: string }[] = [
  {
    id: "claude_code",
    label: "Claude",
    account: "Claude subscription",
    keyName: "Anthropic API key",
    keyHint: "sk-ant-…",
  },
  { id: "codex", label: "Codex", account: "ChatGPT account", keyName: "OpenAI API key", keyHint: "sk-…" },
];

/**
 * How the host's agents sign in to their providers. The host keeps every
 * credential: a key typed here goes to the host's Keychain once and never
 * comes back. Signing in and keys are this computer's host only; a remote
 * host shows its status.
 */
export function AgentsSettings({ remote }: { remote: boolean }) {
  const controller = useController();
  const online = useOnline();
  const [agent, setAgent] = useState<Agent>("claude_code");
  const [statuses, setStatuses] = useState<AgentAuthStatus[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [key, setKey] = useState("");
  const [confirmSignOut, setConfirmSignOut] = useState(false);

  const refresh = useCallback(async () => {
    setStatuses(await controller.client.request("agent.auth_status", {}));
  }, [controller]);

  useEffect(() => {
    refresh().catch((e) => setError(errorMessage(e)));
  }, [refresh]);

  const status = statuses?.find((s) => s.agent === agent) ?? null;
  const info = AGENTS.find((a) => a.id === agent)!;
  const signingIn = statuses?.some((s) => s.sign_in === "running") ?? false;

  // A sign-in page is open: watch for it to finish.
  useEffect(() => {
    if (!signingIn) return;
    const t = setInterval(() => void refresh().catch(() => {}), POLL_MS);
    return () => clearInterval(t);
  }, [signingIn, refresh]);

  function choose(next: Agent) {
    setAgent(next);
    setKey("");
    setError(null);
    setConfirmSignOut(false);
  }

  async function run(label: string, f: () => Promise<AgentAuthStatus | void>) {
    setBusy(label);
    setError(null);
    try {
      const next = await f();
      if (next) setStatuses((all) => all?.map((s) => (s.agent === next.agent ? next : s)) ?? [next]);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  }

  const signIn = () => run("sign-in", () => controller.client.request("agent.sign_in", { agent }));
  const saveKey = () =>
    run("save-key", async () => {
      const next = await controller.client.request("agent.save_key", { agent, key });
      setKey("");
      return next;
    });
  const removeKey = () => run("remove-key", () => controller.client.request("agent.remove_key", { agent }));
  const signOut = () =>
    run("sign-out", async () => {
      setConfirmSignOut(false);
      return controller.client.request("agent.sign_out", { agent });
    });

  const disabled = !online || busy !== null;
  // Signed in with the account already: nothing to sign in to.
  const accountSignedIn = status?.connected === true && status.method === "account" && status.sign_in !== "running";
  return (
    <>
      <div className="flex items-center gap-2">
        <div className="bg-muted flex gap-0.5 rounded-lg p-0.5" role="tablist" aria-label="Agent">
          {AGENTS.map((a) => (
            <button
              key={a.id}
              role="tab"
              aria-selected={a.id === agent}
              className={cn(
                "text-ui rounded-md px-3 py-1 transition-colors",
                a.id === agent ? "bg-background text-foreground shadow-sm" : "text-muted-foreground hover:text-foreground",
              )}
              onClick={() => choose(a.id)}
            >
              {a.label}
            </button>
          ))}
        </div>
        <Button
          className="ml-auto"
          size="xs"
          variant="ghost"
          disabled={disabled}
          onClick={() => void run("refresh", refresh)}
        >
          <RefreshCw className={cn(busy === "refresh" && "animate-spin")} /> Refresh
        </Button>
      </div>

      {statuses === null ? (
        error ? null : <Loader2 className="text-muted-foreground size-4 animate-spin" />
      ) : status ? (
        <>
          <StatusRow status={status} account={info.account} />

          {remote ? (
            <p className="text-muted-foreground" data-testid="agent-remote">
              Sign in on the Mac running Zwiim.
            </p>
          ) : (
            <>
              <div className="flex flex-col gap-1.5 rounded-md border p-3" data-testid="agent-account">
                <div className="flex items-center gap-2">
                  <span className="font-medium">Account</span>
                  {!accountSignedIn && (
                    <Button
                      className="ml-auto"
                      size="xs"
                      variant="outline"
                      disabled={disabled}
                      onClick={() => void signIn()}
                    >
                      {busy === "sign-in" && <Loader2 className="animate-spin" />}
                      {status.sign_in === "running" ? "Start over" : "Sign in"}
                    </Button>
                  )}
                </div>
                <p className="text-muted-foreground">
                  {accountSignedIn
                    ? `Signed in with your ${info.account}.`
                    : status.sign_in === "running"
                      ? `Finish signing in to your ${info.account} in the browser.`
                      : status.sign_in === "failed"
                        ? "The last sign-in did not finish. Try again."
                        : `Sign in with your ${info.account}. The browser on this Mac opens.`}
                </p>
              </div>

              <div className="flex flex-col gap-1.5 rounded-md border p-3" data-testid="agent-key">
                <div className="flex items-center gap-2">
                  <span className="font-medium">API key</span>
                  {status.saved_key && <span className="text-muted-foreground">Saved on this Mac</span>}
                  {status.saved_key && (
                    <Button
                      className="ml-auto"
                      size="xs"
                      variant="ghost"
                      disabled={disabled}
                      onClick={() => void removeKey()}
                    >
                      {busy === "remove-key" && <Loader2 className="animate-spin" />}
                      {busy === "remove-key" ? "Removing…" : "Remove key"}
                    </Button>
                  )}
                </div>
                <form
                  className="flex items-center gap-1.5"
                  onSubmit={(e) => {
                    e.preventDefault();
                    if (key.trim()) void saveKey();
                  }}
                >
                  <input
                    className="bg-input/30 min-w-0 flex-1 rounded-md border px-2 py-1 font-mono text-xs"
                    type="password"
                    aria-label={info.keyName}
                    placeholder={info.keyHint}
                    autoComplete="off"
                    spellCheck={false}
                    value={key}
                    onChange={(e) => setKey(e.target.value)}
                  />
                  <Button size="xs" variant="outline" type="submit" disabled={disabled || !key.trim()}>
                    {busy === "save-key" && <Loader2 className="animate-spin" />}
                    {busy === "save-key" ? "Saving…" : status.saved_key ? "Replace key" : "Save key"}
                  </Button>
                </form>
                <p className="text-muted-foreground">
                  Kept in this Mac's Keychain. Used only while no {info.account} is signed in.
                </p>
              </div>

              {(status.connected || status.saved_key) && (
                <div className="flex items-center gap-2">
                  {confirmSignOut ? (
                    <>
                      <span className="text-muted-foreground flex-1">
                        Sign {info.label} out on this Mac? Its saved key is deleted and its running sessions stop.
                      </span>
                      <Button size="xs" variant="ghost" disabled={disabled} onClick={() => setConfirmSignOut(false)}>
                        Cancel
                      </Button>
                      <Button size="xs" variant="destructive" disabled={disabled} onClick={() => void signOut()}>
                        Sign out
                      </Button>
                    </>
                  ) : (
                    <Button
                      size="xs"
                      variant="destructive"
                      disabled={disabled}
                      onClick={() => setConfirmSignOut(true)}
                    >
                      {busy === "sign-out" && <Loader2 className="animate-spin" />}
                      {busy === "sign-out" ? "Signing out…" : "Sign out"}
                    </Button>
                  )}
                </div>
              )}
            </>
          )}
        </>
      ) : null}

      {error && (
        <p className="text-destructive" role="alert">
          {error}
        </p>
      )}
    </>
  );
}

const METHODS = { api_key: "API key", cloud: "Cloud provider" } as const;

function StatusRow({ status, account }: { status: AgentAuthStatus; account: string }) {
  const method = status.method === "account" ? account : status.method ? METHODS[status.method] : null;
  return (
    <div className="flex items-start gap-2 border-b pb-3" data-testid="agent-status">
      <span
        className={cn("mt-1 size-2 shrink-0 rounded-full", status.connected ? "bg-done" : "bg-faint")}
        aria-hidden
      />
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="text-ui font-medium">{status.connected ? "Connected" : "Not signed in"}</span>
        {method && <span className="text-muted-foreground">{method}</span>}
        {(status.email || status.plan) && (
          <span className="text-muted-foreground truncate font-mono">
            {[status.email, status.plan].filter(Boolean).join(" · ")}
          </span>
        )}
      </div>
    </div>
  );
}
