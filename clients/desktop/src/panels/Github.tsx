import { useCallback, useEffect, useRef, useState } from "react";
import { ArrowUpRight, Check, Copy, Loader2 } from "lucide-react";
import { hasCapability, type GithubAuth } from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import { openLink } from "@/lib/bridge";
import { errorMessage, useController, useHost, useOnline } from "@/lib/host-context";

/** While a sign-in waits for its code to be entered, how often to look for the result. */
export const GITHUB_POLL_MS = 2000;

/** Looks in a row that fail before saying the Host can't be reached. */
const FAILED_POLLS_SHOWN = 3;

/** Whether the Host signs its GitHub CLI in itself (`github.auth`); an older Host asks for `gh auth login`. */
export function useGithubSignInOffered(): boolean {
  return useHost((s) => hasCapability(s.connection.compat, "github.auth"));
}

/**
 * Signing the Host's GitHub CLI in through GitHub's device flow: the Host
 * runs gh, this shows gh's one-time code to enter at the address gh names,
 * then follows the Host until the sign-in settles. `checkFirst` reads the
 * Host's state at once (Settings); without it the button comes first (a
 * "not signed in" message). `onSignedIn` runs once a sign-in started here
 * ends signed in.
 */
export function GithubSignIn({
  host = "the host",
  checkFirst = false,
  onSignedIn,
}: {
  host?: string;
  checkFirst?: boolean;
  onSignedIn?(): void;
}) {
  const controller = useController();
  const online = useOnline();
  const [auth, setAuth] = useState<GithubAuth | null>(null);
  const [busy, setBusy] = useState<"sign-in" | "cancel" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  // Looks at the sign-in in a row that failed; the next that answers resets it.
  const [failedPolls, setFailedPolls] = useState(0);
  const started = useRef(false);
  const onSignedInRef = useRef(onSignedIn);
  onSignedInRef.current = onSignedIn;

  const settle = useCallback((next: GithubAuth) => {
    setAuth(next);
    if (next.state !== "signing_in") setCopied(false);
    if (next.state === "signed_in" && started.current) {
      started.current = false;
      onSignedInRef.current?.();
    }
  }, []);

  const refresh = useCallback(async () => settle(await controller.githubAuthStatus()), [controller, settle]);

  useEffect(() => {
    if (checkFirst) refresh().catch((e) => setError(errorMessage(e)));
  }, [checkFirst, refresh]);

  // gh waits for the code: watch for the sign-in to settle.
  const signingIn = auth?.state === "signing_in";
  useEffect(() => {
    if (!signingIn) return;
    setFailedPolls(0);
    const t = setInterval(
      () =>
        void refresh().then(
          () => setFailedPolls(0),
          () => setFailedPolls((n) => n + 1),
        ),
      GITHUB_POLL_MS,
    );
    return () => clearInterval(t);
  }, [signingIn, refresh]);

  async function run(label: "sign-in" | "cancel", f: () => Promise<GithubAuth>) {
    setBusy(label);
    setError(null);
    // An error from the last attempt is shown once.
    setAuth((a) => a && { ...a, error: undefined });
    try {
      settle(await f());
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  }

  const signIn = () => {
    started.current = true;
    return run("sign-in", () => controller.githubSignIn());
  };
  const cancel = () => {
    started.current = false;
    return run("cancel", () => controller.cancelGithubSignIn());
  };
  const disabled = !online || busy !== null;
  const alert = error ?? auth?.error ?? null;

  const signInButton = (
    <Button size="xs" variant="outline" className="self-start" disabled={disabled} onClick={() => void signIn()}>
      {busy === "sign-in" && <Loader2 className="animate-spin" />}
      Sign in to GitHub
    </Button>
  );

  return (
    <div className="flex flex-col gap-2" data-testid="github-sign-in">
      {auth === null ? (
        checkFirst ? (
          !error && <Loader2 className="text-muted-foreground size-4 animate-spin" aria-label="Loading" />
        ) : (
          signInButton
        )
      ) : auth.state === "signed_in" ? (
        <p className="flex items-center gap-2 text-xs" data-testid="github-signed-in">
          <span className="bg-done size-2 shrink-0 rounded-full" aria-hidden />
          <span>
            Signed in to GitHub as <span className="font-mono">{auth.login ?? "unknown"}</span>
          </span>
        </p>
      ) : auth.state === "unavailable" ? (
        <p className="text-muted-foreground text-xs">GitHub CLI isn't installed on {host}.</p>
      ) : auth.state === "signing_in" && auth.code && auth.url ? (
        <div className="flex flex-col gap-2 rounded-md border p-3" data-testid="github-code">
          <p className="text-muted-foreground text-xs">Enter this code on GitHub to sign {host} in:</p>
          <div className="flex flex-wrap items-center gap-2">
            <code className="font-mono text-2xl font-medium tracking-widest select-all" aria-label="Sign-in code">
              {auth.code}
            </code>
            <Button
              size="xs"
              variant="ghost"
              title="Copy code"
              onClick={() => void navigator.clipboard?.writeText(auth.code!).then(() => setCopied(true))}
            >
              {copied ? <Check /> : <Copy />} {copied ? "Copied" : "Copy"}
            </Button>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <Button size="xs" variant="brand" onClick={() => void openLink(auth.url!).catch(() => {})}>
              Open GitHub
              <ArrowUpRight />
            </Button>
            <Button size="xs" variant="ghost" disabled={disabled} onClick={() => void cancel()}>
              {busy === "cancel" && <Loader2 className="animate-spin" />}
              Cancel
            </Button>
            <span className="text-muted-foreground ml-auto flex items-center gap-1 text-xs" role="status">
              <Loader2 className="size-3 animate-spin" aria-hidden />
              Waiting for GitHub…
            </span>
          </div>
          {failedPolls >= FAILED_POLLS_SHOWN && (
            <p className="text-destructive text-xs" data-testid="github-poll-failed">
              Not connected to the Host
            </p>
          )}
        </div>
      ) : (
        signInButton
      )}
      {alert && (
        <p className="text-destructive text-xs" role="alert">
          {alert}
        </p>
      )}
    </div>
  );
}

/** Settings → GitHub: how the Host's GitHub CLI is signed in, and signing it in. */
export function GithubSettings({ remote }: { remote: boolean }) {
  const host = remote ? "the host" : "this Mac";
  return (
    <>
      <p className="text-muted-foreground">
        Zwiim uses the GitHub CLI on {host} to publish, clone and follow pull requests. Signing in opens GitHub in this
        computer's browser; the account stays with {host}.
      </p>
      <GithubSignIn host={host} checkFirst />
    </>
  );
}
