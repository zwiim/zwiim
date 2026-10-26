// Signing the Host's GitHub CLI in from the phone: the Host runs gh's device
// flow, the phone shows gh's one-time code and opens GitHub to enter it, then
// follows the Host until the sign-in settles. No token ever reaches the phone.

import {
  hasCapability,
  type Compatibility,
  type GithubAuth,
  type OpName,
  type OpParams,
  type OpResult,
} from "@zwiim/sdk";
import { errorInfo } from "./connection";

/** While a sign-in waits for its code to be entered, how often to look for the result. */
export const GITHUB_POLL_MS = 2000;

/** `github.sign_in` can take about 55 s at worst (three gh checks and the wait for its code); reads and Cancel answer at once. */
const SIGN_IN_TIMEOUT_MS = 60_000;
const READ_TIMEOUT_MS = 20_000;

/** Whether the Host signs its GitHub CLI in itself; an older Host asks for `gh auth login`. */
export function githubSignInOffered(compat: Compatibility | null): boolean {
  return hasCapability(compat, "github.auth");
}

/** Where a `gh_unauthenticated` message sends the user on a Host that signs in itself. */
export const SIGN_IN_HERE = "Sign in under GitHub in the Host tab";

/** Only GitHub's own https page opens from here, whatever the Host sends. */
export function githubPage(url: string | undefined): string | null {
  if (!url) return null;
  try {
    const u = new URL(url);
    return u.protocol === "https:" && u.hostname === "github.com" ? url : null;
  } catch {
    return null;
  }
}

export interface GithubHost {
  request<O extends OpName>(op: O, params: OpParams<O>, opts?: { timeoutMs?: number }): Promise<OpResult<O>>;
}

export interface GithubSignInState {
  auth: GithubAuth | null;
  busy: "sign-in" | "cancel" | null;
  /** Why the last call failed, or why the last sign-in didn't finish. Shown once. */
  error: string | null;
}

interface Timers {
  setInterval(f: () => void, ms: number): unknown;
  clearInterval(t: unknown): void;
}

const realTimers: Timers = {
  setInterval: (f, ms) => setInterval(f, ms),
  clearInterval: (t) => clearInterval(t as ReturnType<typeof setInterval>),
};

/**
 * The GitHub group's state, outside React so it can be tested: reads, starts
 * and cancels the Host's sign-in, and looks every `GITHUB_POLL_MS` while it
 * waits for the code to be entered.
 */
export class GithubSignInFlow {
  state: GithubSignInState = { auth: null, busy: null, error: null };
  readonly #host: GithubHost;
  readonly #hostName: string;
  readonly #timers: Timers;
  readonly #listeners = new Set<() => void>();
  #poll: unknown = null;
  #disposed = false;

  constructor(host: GithubHost, hostName: string, timers: Timers = realTimers) {
    this.#host = host;
    this.#hostName = hostName;
    this.#timers = timers;
  }

  subscribe = (f: () => void): (() => void) => {
    this.#listeners.add(f);
    return () => this.#listeners.delete(f);
  };

  /** Read the Host's state; a failed read keeps what was shown. */
  async refresh(): Promise<void> {
    try {
      this.#settle(await this.#host.request("github.auth_status", {}, { timeoutMs: READ_TIMEOUT_MS }));
    } catch {
      // The next look, or the user's next tap, tries again.
    }
  }

  signIn(): Promise<void> {
    return this.#run("sign-in", () => this.#host.request("github.sign_in", {}, { timeoutMs: SIGN_IN_TIMEOUT_MS }));
  }

  cancel(): Promise<void> {
    return this.#run("cancel", () => this.#host.request("github.cancel_sign_in", {}, { timeoutMs: READ_TIMEOUT_MS }));
  }

  /** Stop looking. The Host's sign-in goes on; opening the group again shows its code. */
  dispose(): void {
    this.#disposed = true;
    this.#stopPolling();
    this.#listeners.clear();
  }

  async #run(busy: "sign-in" | "cancel", f: () => Promise<GithubAuth>): Promise<void> {
    if (this.state.busy) return;
    const auth = this.state.auth && { ...this.state.auth, error: undefined };
    this.#set({ auth, busy, error: null });
    try {
      const next = await f();
      this.#set({ busy: null });
      this.#settle(next);
    } catch (e) {
      this.#set({ busy: null, error: this.#problem(e) });
    }
  }

  #settle(auth: GithubAuth) {
    if (this.#disposed) return;
    this.#set({ auth, error: auth.error ?? null });
    if (auth.state === "signing_in") this.#startPolling();
    else this.#stopPolling();
  }

  #problem(e: unknown): string {
    const { code } = errorInfo(e);
    if (code === "network" || code === "timeout")
      return `Not connected to ${this.#hostName}. Try again when it's back.`;
    return "Couldn't sign in to GitHub. Try again.";
  }

  #startPolling() {
    if (this.#poll !== null) return;
    this.#poll = this.#timers.setInterval(() => void this.refresh(), GITHUB_POLL_MS);
  }

  #stopPolling() {
    if (this.#poll === null) return;
    this.#timers.clearInterval(this.#poll);
    this.#poll = null;
  }

  #set(patch: Partial<GithubSignInState>) {
    if (this.#disposed) return;
    this.state = { ...this.state, ...patch };
    for (const f of this.#listeners) f();
  }
}
