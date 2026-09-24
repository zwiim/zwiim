import { Loader2, ShieldAlert, UserRound } from "lucide-react";
import { Button } from "@/components/ui/button";
import type { AccountState } from "@/lib/account";
import { useHost } from "@/lib/host-context";

/** Sidebar: who is signed in here, and whose host this is. */
export function AccountPanel({ state }: { state: AccountState }) {
  const owner = useHost((s) => s.host?.owner ?? null);
  const { account, ownership, pending, error, status } = state;
  if (!status) return null;
  return (
    <div className="flex flex-col gap-1 text-xs" data-testid="account">
      {account ? (
        <div className="flex items-center gap-1.5">
          <UserRound className="text-muted-foreground size-3.5 shrink-0" />
          <span className="truncate" title={account.email ?? account.user_id}>
            {account.email ?? "Signed in"}
          </span>
          <button
            className="text-muted-foreground ml-auto shrink-0 underline"
            title="Forget this account on this computer. The host, its agents and sessions keep running."
            onClick={() => void state.signOut()}
          >
            Sign out
          </button>
        </div>
      ) : (
        <div className="flex items-center gap-1.5">
          <span className="text-muted-foreground">Local mode</span>
          {status.available &&
            (pending ? (
              <span className="text-muted-foreground ml-auto flex items-center gap-1">
                <Loader2 className="size-3 animate-spin" /> In browser…
                <button className="underline" onClick={() => void state.cancelSignIn()}>
                  Cancel
                </button>
              </span>
            ) : (
              <button className="ml-auto underline" onClick={() => void state.signIn()}>
                Sign in
              </button>
            ))}
        </div>
      )}
      {ownership.kind === "owner" && <span className="text-muted-foreground">This host is yours</span>}
      {ownership.kind === "local" && owner && (
        <span className="text-muted-foreground truncate" data-testid="host-owner">
          Host owner: {owner.email ?? "another account"}
        </span>
      )}
      {ownership.kind === "mismatch" && (
        <div
          className="flex gap-1.5 rounded-md border border-amber-500/50 bg-amber-500/10 p-2 text-amber-800 dark:text-amber-300"
          role="status"
          data-testid="owner-mismatch"
        >
          <ShieldAlert className="size-3.5 shrink-0" />
          <span>
            This host belongs to {ownership.ownerEmail ?? "another account"}. It stays theirs; you can keep using it
            locally.
          </span>
        </div>
      )}
      {error && (
        <span className="text-destructive" role="alert">
          {error}
        </span>
      )}
    </div>
  );
}

/** First run, when this build can sign in: Google or local, never forced. */
export function Welcome({ state, onLocal }: { state: AccountState; onLocal(): void }) {
  return (
    <div className="bg-background/80 absolute inset-0 z-20 flex items-center justify-center backdrop-blur-sm" data-testid="welcome">
      <div className="bg-card flex w-80 flex-col gap-3 rounded-lg border p-6 shadow-lg">
        <p className="text-lg font-semibold">Welcome to Zwiim</p>
        <p className="text-muted-foreground text-xs">
          Sign in to link this computer's host to your account. Everything runs here either way.
        </p>
        <Button onClick={() => void state.signIn()} disabled={state.pending}>
          {state.pending && <Loader2 className="animate-spin" />}
          {state.pending ? "Finish signing in in your browser…" : "Continue with Google"}
        </Button>
        {state.pending ? (
          <Button variant="ghost" onClick={() => void state.cancelSignIn()}>
            Cancel
          </Button>
        ) : (
          <Button variant="outline" onClick={onLocal}>
            Continue locally
          </Button>
        )}
        {state.error && (
          <p className="text-destructive text-xs" role="alert">
            {state.error}
          </p>
        )}
      </div>
    </div>
  );
}
