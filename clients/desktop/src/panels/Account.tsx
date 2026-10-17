import { Loader2, ShieldAlert, UserRound } from "lucide-react";
import { Button } from "@/components/ui/button";
import type { AccountState } from "@/lib/account";
import { useHost } from "@/lib/host-context";
import { cn } from "@/lib/utils";

/** Settings › Account: who is signed in here, and whose host this is. */
export function AccountPanel({ state }: { state: AccountState }) {
  const owner = useHost((s) => s.host?.owner ?? null);
  const { account, ownership, pending, error, status } = state;
  if (!status) return null;
  return (
    <div className="flex flex-col gap-3" data-testid="account">
      <div className="flex items-center gap-3">
        <Avatar email={account?.email ?? null} size="lg" />
        <div className="flex min-w-0 flex-1 flex-col">
          <span className="text-ui truncate font-medium" title={account ? (account.email ?? account.user_id) : undefined}>
            {account ? (account.email ?? "Signed in") : "Not signed in"}
          </span>
          <span className="text-muted-foreground text-xs">
            {account
              ? "Signed in with Google"
              : status.available
                ? "Everything runs on this computer. Sign in to reach it from your other devices."
                : "Everything runs on this computer."}
          </span>
        </div>
        {account ? (
          <Button
            size="sm"
            variant="outline"
            title="Forget this account on this computer. The host, its agents and sessions keep running."
            onClick={() => void state.signOut()}
          >
            Sign out
          </Button>
        ) : (
          status.available &&
          (pending ? (
            <span className="text-muted-foreground flex items-center gap-1.5 text-xs">
              <Loader2 className="size-3.5 animate-spin" /> In browser…
              <Button size="sm" variant="ghost" onClick={() => void state.cancelSignIn()}>
                Cancel
              </Button>
            </span>
          ) : (
            <Button size="sm" variant="brand" onClick={() => void state.signIn()}>
              Sign in
            </Button>
          ))
        )}
      </div>
      {ownership.kind === "owner" && <p className="text-muted-foreground text-xs">This host is yours</p>}
      {ownership.kind === "local" && owner && (
        <p className="text-muted-foreground truncate text-xs" data-testid="host-owner">
          Host owner: {owner.email ?? "another account"}
        </p>
      )}
      {ownership.kind === "mismatch" && (
        <div
          className="border-brand/40 bg-brand-soft text-foreground flex gap-2 rounded-md border p-2 text-xs"
          role="status"
          data-testid="owner-mismatch"
        >
          <ShieldAlert className="text-brand size-3.5 shrink-0" />
          <span>
            This host belongs to {ownership.ownerEmail ?? "another account"}. It stays theirs; you can keep using it
            locally.
          </span>
        </div>
      )}
      {error && (
        <span className="text-destructive text-xs" role="alert">
          {error}
        </span>
      )}
    </div>
  );
}

/** The account's initial, or a person when no one is signed in. */
export function Avatar({ email, size = "sm" }: { email: string | null; size?: "sm" | "lg" }) {
  const initial = email?.trim()[0]?.toUpperCase();
  return (
    <span
      className={cn(
        "bg-brand text-brand-foreground grid shrink-0 place-items-center rounded-full font-semibold",
        size === "lg" ? "size-9 text-sm" : "size-7 text-xs",
        !initial && "bg-muted text-muted-foreground",
      )}
      aria-hidden
    >
      {initial ?? <UserRound className={size === "lg" ? "size-4" : "size-3.5"} />}
    </span>
  );
}

/** First run, when this build can sign in: Google or local, never forced. */
export function Welcome({ state, onLocal }: { state: AccountState; onLocal(): void }) {
  return (
    <div className="bg-background/80 absolute inset-0 z-20 flex items-center justify-center backdrop-blur-sm" data-testid="welcome">
      <div className="bg-popover shadow-pop animate-enter flex w-80 flex-col gap-3 rounded-xl border p-6">
        <p className="text-lg font-semibold">Welcome to Zwiim</p>
        <p className="text-muted-foreground text-xs">
          Sign in to link this computer's host to your account. Everything runs here either way.
        </p>
        <Button variant="brand" onClick={() => void state.signIn()} disabled={state.pending}>
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
