import { describe, expect, it } from "vitest";
import { ZwiimClientError, type Compatibility, type GithubAuth } from "@zwiim/sdk";
import { GITHUB_POLL_MS, GithubSignInFlow, githubPage, githubSignInOffered, type GithubHost } from "../src/host/github";

const CODE: GithubAuth = { state: "signing_in", code: "ABCD-1234", url: "https://github.com/login/device" };

/** A Host whose GitHub ops answer from `handlers`, recording each call. */
function fakeHost(handlers: Record<string, () => GithubAuth | Promise<GithubAuth>>) {
  const calls: { op: string; params: unknown; timeoutMs?: number }[] = [];
  const host: GithubHost = {
    request: (async (op: string, params: unknown, opts?: { timeoutMs?: number }) => {
      calls.push({ op, params, timeoutMs: opts?.timeoutMs });
      const h = handlers[op];
      if (!h) throw new ZwiimClientError("unknown_op", op, 404);
      return h();
    }) as GithubHost["request"],
  };
  return { host, calls, count: (op: string) => calls.filter((c) => c.op === op).length };
}

/** Timers the test advances by hand. */
function fakeTimers() {
  const live = new Map<number, { f: () => void; ms: number }>();
  let next = 1;
  return {
    timers: {
      setInterval: (f: () => void, ms: number) => {
        live.set(next, { f, ms });
        return next++;
      },
      clearInterval: (t: unknown) => void live.delete(t as number),
    },
    live,
    /** Run every live interval once, as if `GITHUB_POLL_MS` had passed. */
    async tick() {
      for (const t of [...live.values()]) t.f();
      await flush();
    },
  };
}

const flush = () => new Promise((r) => setTimeout(r, 0));

describe("GithubSignInFlow", () => {
  it("reads how the Host's GitHub CLI is signed in", async () => {
    const h = fakeHost({ "github.auth_status": () => ({ state: "signed_in", login: "Ashish1101" }) });
    const flow = new GithubSignInFlow(h.host, "Studio Mac", fakeTimers().timers);
    await flow.refresh();
    expect(flow.state).toEqual({ auth: { state: "signed_in", login: "Ashish1101" }, busy: null, error: null });
    expect(h.calls).toEqual([{ op: "github.auth_status", params: {}, timeoutMs: 20_000 }]);
  });

  it("keeps what it showed when a read fails", async () => {
    let fail = false;
    const h = fakeHost({
      "github.auth_status": () => {
        if (fail) throw new ZwiimClientError("network", "Not connected to the host");
        return { state: "signed_out" };
      },
    });
    const flow = new GithubSignInFlow(h.host, "Studio Mac", fakeTimers().timers);
    await flow.refresh();
    fail = true;
    await flow.refresh();
    expect(flow.state).toEqual({ auth: { state: "signed_out" }, busy: null, error: null });
  });

  it("signs in: shows the code, looks every 2 s, and stops once it settles", async () => {
    let state: GithubAuth = { state: "signed_out" };
    const h = fakeHost({
      "github.auth_status": () => state,
      "github.sign_in": () => (state = CODE),
    });
    const t = fakeTimers();
    const flow = new GithubSignInFlow(h.host, "Studio Mac", t.timers);
    const seen: (string | null)[] = [];
    flow.subscribe(() => seen.push(flow.state.busy));
    await flow.refresh();

    const signing = flow.signIn();
    expect(flow.state.busy).toBe("sign-in");
    await signing;
    expect(h.calls.find((c) => c.op === "github.sign_in")).toEqual({
      op: "github.sign_in",
      params: {},
      timeoutMs: 60_000,
    });
    expect(flow.state).toEqual({ auth: CODE, busy: null, error: null });
    expect(seen).toContain("sign-in");
    expect([...t.live.values()].map((x) => x.ms)).toEqual([GITHUB_POLL_MS]);
    expect(GITHUB_POLL_MS).toBe(2000);

    const before = h.count("github.auth_status");
    await t.tick();
    await t.tick();
    expect(h.count("github.auth_status")).toBe(before + 2);
    // Still waiting: one poll, not one per look.
    expect(t.live.size).toBe(1);

    state = { state: "signed_in", login: "Ashish1101" };
    await t.tick();
    expect(flow.state.auth).toEqual({ state: "signed_in", login: "Ashish1101" });
    expect(t.live.size).toBe(0);
  });

  it("answers signed in at once without polling when already signed in", async () => {
    const h = fakeHost({ "github.sign_in": () => ({ state: "signed_in", login: "Ashish1101" }) });
    const t = fakeTimers();
    const flow = new GithubSignInFlow(h.host, "Studio Mac", t.timers);
    await flow.signIn();
    expect(flow.state.auth?.state).toBe("signed_in");
    expect(t.live.size).toBe(0);
  });

  it("Cancel stops the sign-in and the looking", async () => {
    let state: GithubAuth = CODE;
    const h = fakeHost({
      "github.auth_status": () => state,
      "github.cancel_sign_in": () => (state = { state: "signed_out" }),
    });
    const t = fakeTimers();
    const flow = new GithubSignInFlow(h.host, "Studio Mac", t.timers);
    // A sign-in already under way shows its code again, and is followed.
    await flow.refresh();
    expect(flow.state.auth).toEqual(CODE);
    expect(t.live.size).toBe(1);

    await flow.cancel();
    expect(h.calls.find((c) => c.op === "github.cancel_sign_in")).toEqual({
      op: "github.cancel_sign_in",
      params: {},
      timeoutMs: 20_000,
    });
    expect(flow.state).toEqual({ auth: { state: "signed_out" }, busy: null, error: null });
    expect(t.live.size).toBe(0);
  });

  it("shows why a sign-in didn't finish, once, and lets it start again", async () => {
    let answer: GithubAuth = { state: "signed_out", error: "GitHub didn't give a sign-in code. Try again." };
    const h = fakeHost({ "github.sign_in": () => answer });
    const flow = new GithubSignInFlow(h.host, "Studio Mac", fakeTimers().timers);
    await flow.signIn();
    expect(flow.state.error).toBe("GitHub didn't give a sign-in code. Try again.");

    // The next attempt starts clean: the old error is gone while it runs.
    let during: string | null | undefined;
    const sub = flow.subscribe(() => {
      if (flow.state.busy) during = flow.state.error ?? flow.state.auth?.error;
    });
    answer = CODE;
    await flow.signIn();
    sub();
    expect(during).toBeUndefined();
    expect(flow.state).toEqual({ auth: CODE, busy: null, error: null });
  });

  it("says when the phone lost the Host, and when the Host refused", async () => {
    const lost = fakeHost({
      "github.sign_in": () => {
        throw new ZwiimClientError("network", "Not connected to the host");
      },
    });
    const flow = new GithubSignInFlow(lost.host, "Studio Mac", fakeTimers().timers);
    await flow.signIn();
    expect(flow.state.error).toBe("Not connected to Studio Mac. Try again when it's back.");
    expect(flow.state.busy).toBeNull();

    const refused = fakeHost({
      "github.sign_in": () => {
        throw new ZwiimClientError("internal", "gh exploded: secret detail", 500);
      },
    });
    const other = new GithubSignInFlow(refused.host, "Studio Mac", fakeTimers().timers);
    await other.signIn();
    expect(other.state.error).toBe("Couldn't sign in to GitHub. Try again.");
  });

  it("ignores a second tap while a call is out", async () => {
    let release!: (a: GithubAuth) => void;
    const h = fakeHost({ "github.sign_in": () => new Promise<GithubAuth>((r) => (release = r)) });
    const flow = new GithubSignInFlow(h.host, "Studio Mac", fakeTimers().timers);
    const first = flow.signIn();
    await flow.signIn();
    await flow.cancel();
    release(CODE);
    await first;
    expect(h.calls.map((c) => c.op)).toEqual(["github.sign_in"]);
  });

  it("stops looking once the group closes", async () => {
    const h = fakeHost({ "github.auth_status": () => CODE });
    const t = fakeTimers();
    const flow = new GithubSignInFlow(h.host, "Studio Mac", t.timers);
    await flow.refresh();
    expect(t.live.size).toBe(1);
    flow.dispose();
    expect(t.live.size).toBe(0);
    await flow.refresh();
    expect(t.live.size).toBe(0);
  });
});

describe("githubPage", () => {
  it("opens only GitHub's own https page", () => {
    expect(githubPage("https://github.com/login/device")).toBe("https://github.com/login/device");
    expect(githubPage("http://github.com/login/device")).toBeNull();
    expect(githubPage("https://github.com.evil.example/login/device")).toBeNull();
    expect(githubPage("https://evil.example/?github.com")).toBeNull();
    expect(githubPage("javascript:alert(1)")).toBeNull();
    expect(githubPage("not a url")).toBeNull();
    expect(githubPage(undefined)).toBeNull();
  });
});

describe("githubSignInOffered", () => {
  const serving = (capabilities: string[]): Compatibility => ({ status: "compatible", hostRevision: 9, capabilities });

  it("is offered by a Host with github.auth, and not by an older one", () => {
    expect(githubSignInOffered(serving(["github.auth", "terminal"]))).toBe(true);
    expect(githubSignInOffered(serving(["terminal"]))).toBe(false);
    expect(githubSignInOffered(null)).toBe(false);
  });
});
