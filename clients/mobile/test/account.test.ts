import { CloudClient, encodeKey, isClientError } from "@zwiim/sdk";
import { sha256 } from "@noble/hashes/sha2.js";
import { describe, expect, it } from "vitest";
import { AccountAuth, AUTH_REDIRECT, authConfig, SignInCancelled, tokenExpiry } from "../src/account/auth";
import { accountHosts, byLastUsed, hostMeta, hostName, type HostEntry } from "../src/account/hosts";
import { appConfig } from "../src/config";
import { MemorySecureStore } from "../src/platform/memory-store";
import { FakeCloud, FakeHost } from "../../sdk/test/relay-fakes";
import { ANON_KEY, FakeSupabase, jwt, SUPABASE } from "./auth-fakes";

function setup() {
  const supabase = new FakeSupabase();
  const store = new MemorySecureStore();
  const auth = new AccountAuth({
    config: { url: SUPABASE, anonKey: ANON_KEY },
    store,
    openAuthSession: supabase.openAuthSession,
    fetch: supabase.fetch,
    now: () => supabase.now,
  });
  return { supabase, store, auth };
}

describe("configuration", () => {
  it("needs a Cloud and a Supabase project, over HTTPS or loopback", () => {
    const env = {
      EXPO_PUBLIC_ZWIIM_CLOUD_URL: "https://cloud.example.com",
      EXPO_PUBLIC_ZWIIM_SUPABASE_URL: "https://abc.supabase.co/",
      EXPO_PUBLIC_ZWIIM_SUPABASE_ANON_KEY: "anon",
    };
    expect(appConfig(env)).toEqual({
      cloudUrl: "https://cloud.example.com",
      auth: { url: "https://abc.supabase.co", anonKey: "anon" },
    });
    expect(appConfig({ ...env, EXPO_PUBLIC_ZWIIM_SUPABASE_ANON_KEY: undefined })).toBeNull();
    expect(appConfig({ ...env, EXPO_PUBLIC_ZWIIM_CLOUD_URL: "" })).toBeNull();
    expect(authConfig("http://abc.supabase.co", "anon")).toBeNull();
    expect(authConfig("http://127.0.0.1:54321", "anon")).toEqual({ url: "http://127.0.0.1:54321", anonKey: "anon" });
  });
});

describe("signing in", () => {
  it("runs Google through Supabase with PKCE and keeps the session", async () => {
    const { supabase, store, auth } = setup();
    expect(await auth.current()).toBeNull();
    const account = await auth.signIn();
    expect(account).toEqual({ user_id: "u1", email: "u1@example.com" });

    const opened = supabase.lastAuthorize!;
    expect(opened.origin + opened.pathname).toBe(`${SUPABASE}/auth/v1/authorize`);
    expect(opened.searchParams.get("provider")).toBe("google");
    expect(opened.searchParams.get("redirect_to")).toBe(AUTH_REDIRECT);
    expect(opened.searchParams.get("code_challenge_method")).toBe("s256");
    // The verifier sent with the code is the challenge's.
    const exchange = supabase.calls.find((c) => c.grant === "pkce")!;
    expect(encodeKey(sha256(new TextEncoder().encode(exchange.body.code_verifier!)))).toBe(
      opened.searchParams.get("code_challenge"),
    );

    expect(await store.accountToken()).toBe(supabase.issued[0]);
    expect(await store.accountSession()).toEqual({ user_id: "u1", email: "u1@example.com", refresh_token: "refresh-2" });
    expect(await auth.current()).toEqual(account);
  });

  it("is cancelled when the page is closed, and keeps nothing", async () => {
    const { store, auth } = setup();
    const cancelling = new AccountAuth({
      config: { url: SUPABASE, anonKey: ANON_KEY },
      store,
      openAuthSession: async () => ({ type: "cancel" }),
    });
    await expect(cancelling.signIn()).rejects.toBeInstanceOf(SignInCancelled);
    expect(await auth.current()).toBeNull();
  });

  it("fails on a provider error or a refused exchange", async () => {
    const { supabase, store } = setup();
    const denied = new AccountAuth({
      config: { url: SUPABASE, anonKey: ANON_KEY },
      store,
      openAuthSession: async (_, r) => ({ type: "success", url: `${r}?error=access_denied&error_description=Denied` }),
      fetch: supabase.fetch,
    });
    await expect(denied.signIn()).rejects.toThrow("Sign-in failed: Denied");
    const forged = new AccountAuth({
      config: { url: SUPABASE, anonKey: ANON_KEY },
      store,
      openAuthSession: async (_, r) => ({ type: "success", url: `${r}?code=made-up` }),
      fetch: supabase.fetch,
    });
    await expect(forged.signIn()).rejects.toThrow("refused the sign-in (400)");
    expect(await store.accountSession()).toBeNull();
  });
});

describe("the access token", () => {
  it("is the kept one until it nearly expires, then renewed once for concurrent callers", async () => {
    const { supabase, store, auth } = setup();
    await auth.signIn();
    const first = supabase.issued[0]!;
    expect(await auth.accessToken()).toBe(first);
    expect(tokenExpiry(first)).toBe(supabase.now + 3600);

    supabase.now += 3600 - 30;
    const [a, b] = await Promise.all([auth.accessToken(), auth.accessToken()]);
    expect(a).toBe(supabase.issued[1]);
    expect(b).toBe(a);
    expect(supabase.calls.filter((c) => c.grant === "refresh_token")).toHaveLength(1);
    expect(await store.accountToken()).toBe(a);
    // The rotated refresh token is the one kept.
    expect((await store.accountSession())?.refresh_token).toBe("refresh-3");
    expect(await auth.current()).toEqual({ user_id: "u1", email: "u1@example.com" });
  });

  it("is renewed on demand after the Cloud refused it", async () => {
    const { supabase, auth } = setup();
    await auth.signIn();
    expect(await auth.accessToken(true)).toBe(supabase.issued[1]);
  });

  it("signs out when the renewal is refused", async () => {
    const { supabase, store, auth } = setup();
    await auth.signIn();
    supabase.revokeAll();
    const err = await auth.accessToken(true).catch((e) => e);
    expect(isClientError(err, "unauthenticated")).toBe(true);
    expect(await auth.current()).toBeNull();
    expect(await store.accountToken()).toBeNull();
  });

  it("keeps the session when the sign-in service is down", async () => {
    const { supabase, auth } = setup();
    await auth.signIn();
    supabase.failWith = 503;
    const err = await auth.accessToken(true).catch((e) => e);
    expect(isClientError(err, "network")).toBe(true);
    expect(await auth.current()).not.toBeNull();
  });

  it("is refused when signed out", async () => {
    const { auth } = setup();
    expect(isClientError(await auth.accessToken().catch((e) => e), "unauthenticated")).toBe(true);
  });
});

describe("signing out", () => {
  it("forgets the account, revokes it, and keeps the pairings", async () => {
    const { supabase, store, auth } = setup();
    await auth.signIn();
    await store.pinHost({ host_id: "host-1", host_static_pubkey: "pk" });
    await auth.signOut();
    expect(await auth.current()).toBeNull();
    expect(await store.accountToken()).toBeNull();
    expect(supabase.revoked).toEqual([`Bearer ${supabase.issued[0]}`]);
    expect(await store.pinnedHost("host-1")).not.toBeNull();
  });
});

describe("the account's hosts", () => {
  function hostsWorld() {
    const cloud = new FakeCloud();
    const a = new FakeHost("host-a");
    const b = new FakeHost("host-b");
    cloud.addHost(a, "u1", "Studio Mac");
    cloud.addHost(b, "u1", null);
    cloud.addHost(new FakeHost("host-z"), "u2", "Not mine");
    cloud.setOnline("host-a", false);
    return { cloud, a, b };
  }

  it("lists the account's hosts with presence, online first, marking the paired ones", async () => {
    const { cloud, a } = hostsWorld();
    const store = new MemorySecureStore();
    await store.pinHost({ host_id: "host-a", host_static_pubkey: a.publicKey });
    const client = new CloudClient({ url: cloud.origin, accessToken: "token-u1", fetch: cloud.fetch });
    const hosts = await accountHosts(client, store, async () => {});
    expect(hosts).toEqual([
      { host_id: "host-b", name: null, static_pubkey: expect.any(String), online: true, last_seen_at: 1, paired: false },
      { host_id: "host-a", name: "Studio Mac", static_pubkey: a.publicKey, online: false, last_seen_at: 1, paired: true },
    ]);
    expect(cloud.http.at(-1)).toEqual({ method: "GET", path: "/v1/hosts", authorization: "Bearer token-u1" });
  });

  it("renews the token once when the Cloud refuses it", async () => {
    const { cloud } = hostsWorld();
    let token = "stale";
    let renewals = 0;
    const client = new CloudClient({ url: cloud.origin, accessToken: async () => token, fetch: cloud.fetch });
    const hosts = await accountHosts(client, new MemorySecureStore(), async () => {
      renewals++;
      token = "token-u1";
    });
    expect(renewals).toBe(1);
    expect(hosts).toHaveLength(2);

    token = "stale";
    const err = await accountHosts(client, new MemorySecureStore(), async () => {}).catch((e) => e);
    expect(isClientError(err, "unauthenticated")).toBe(true);
  });

  it("describes each host in its row", () => {
    const now = 100_000;
    const h = (over: Partial<HostEntry>): HostEntry => ({
      host_id: "0123456789abcdef",
      name: null,
      static_pubkey: "pk",
      online: true,
      last_seen_at: now,
      paired: true,
      ...over,
    });
    expect(hostName(h({}))).toBe("Host 01234567");
    expect(hostName(h({ name: " Studio Mac " }))).toBe("Studio Mac");
    expect(hostMeta(h({ paired: false }), now)).toBe("not paired on this phone");
    expect(hostMeta(h({ paired: false }), now, { active: "lan" })).toBe("not paired on this phone");
    // Online by the Cloud's presence: reached through it, unless Local only.
    expect(hostMeta(h({}), now)).toBe("☁ via Zwiim Cloud · paired");
    expect(hostMeta(h({}), now, { mode: "local" })).toBe("online · paired");
    // Only this phone's own live LAN connection says "same network".
    expect(hostMeta(h({}), now, { active: "lan" })).toBe("⌂ same network · paired");
    expect(hostMeta(h({}), now, { mode: "cloud", active: "relay" })).toBe("☁ via Zwiim Cloud · paired");
    expect(hostMeta(h({}), now, { active: "connecting" })).toBe("connecting…");
    expect(hostMeta(h({}), now, { active: "reconnecting" })).toBe("reconnecting…");
    expect(hostMeta(h({ online: false, last_seen_at: now - 7300 }), now)).toBe("offline · last seen 2h ago");
    expect(hostMeta(h({ online: false, last_seen_at: now - 7300 }), now, { active: "offline" })).toBe(
      "offline · last seen 2h ago",
    );
    expect(hostMeta(h({ online: false, last_seen_at: null }), now)).toBe("offline");
  });

  it("puts the last host opened first", () => {
    const row = (host_id: string): HostEntry => ({
      host_id,
      name: null,
      static_pubkey: "pk",
      online: true,
      last_seen_at: 1,
      paired: true,
    });
    const hosts = [row("a"), row("b"), row("c")];
    expect(byLastUsed(hosts, "c").map((h) => h.host_id)).toEqual(["c", "a", "b"]);
    expect(byLastUsed(hosts, "gone").map((h) => h.host_id)).toEqual(["a", "b", "c"]);
    expect(byLastUsed(hosts, null)).toBe(hosts);
  });
});

it("reads a token's expiry only from a JWT", () => {
  expect(tokenExpiry(jwt({ exp: 42 }))).toBe(42);
  expect(tokenExpiry("opaque")).toBeNull();
  expect(tokenExpiry(jwt({ sub: "u1" }))).toBeNull();
});
