// The Zwiim account on this phone: "Continue with Google" through Supabase
// Auth, as the Desktop does it (OAuth PKCE; see
// `clients/desktop/src-tauri/src/account.rs`). The provider page opens in the
// system's auth session and returns to `zwiim://auth-callback` with a code,
// which is exchanged for the account's session.
//
// The access token (the Zwiim Cloud's bearer token) and the refresh token
// live in the secure store only. The access token is renewed when it is
// about to expire; a renewal the service refuses (revoked or lapsed) signs
// out, so the app asks to sign in again. Signing out forgets the account
// here and revokes it upstream when reachable. It never touches this
// device's key or its pairings.

import { encodeKey, ZwiimClientError } from "@zwiim/sdk";
import { sha256 } from "@noble/hashes/sha2.js";
import type { AccountSession, SecureStore } from "../platform/capabilities";

/** Where the provider page returns; Supabase must allow it as a redirect URL. */
export const AUTH_REDIRECT = "zwiim://auth-callback";

export interface AuthConfig {
  /** Supabase project URL, e.g. `https://abc.supabase.co`. */
  url: string;
  /** Public by design; never a private secret. */
  anonKey: string;
}

/** Only HTTPS, or plain HTTP to this machine (a local stand-in). */
export function authConfig(url: string | undefined, anonKey: string | undefined): AuthConfig | null {
  if (!url?.trim() || !anonKey?.trim()) return null;
  let parsed: URL;
  try {
    parsed = new URL(url.trim());
  } catch {
    return null;
  }
  const local = ["127.0.0.1", "localhost", "[::1]"].includes(parsed.hostname);
  if (parsed.protocol !== "https:" && !(parsed.protocol === "http:" && local)) return null;
  return { url: url.trim().replace(/\/+$/, ""), anonKey: anonKey.trim() };
}

/** Who is signed in. Never a token. */
export interface Account {
  user_id: string;
  email: string | null;
}

/** The platform's auth session (`expo-web-browser`'s `openAuthSessionAsync`). */
export type OpenAuthSession = (url: string, redirectUrl: string) => Promise<{ type: string; url?: string }>;

/** The user closed the sign-in page. */
export class SignInCancelled extends Error {
  constructor() {
    super("Sign-in cancelled");
    this.name = "SignInCancelled";
  }
}

const signInRequired = () => new ZwiimClientError("unauthenticated", "Sign in to reach your hosts");
/** Renew a token this close to its expiry. */
const REFRESH_MARGIN_S = 60;

/** A JWT's `exp`, unverified: the service verifies; this only decides when to renew. */
export function tokenExpiry(token: string): number | null {
  const payload = token.split(".")[1];
  if (!payload) return null;
  try {
    const json = atob(payload.replace(/-/g, "+").replace(/_/g, "/") + "=".repeat((4 - (payload.length % 4)) % 4));
    const exp: unknown = JSON.parse(json).exp;
    return typeof exp === "number" ? exp : null;
  } catch {
    return null;
  }
}

/** PKCE verifier and its S256 challenge. */
export function pkce(): { verifier: string; challenge: string } {
  const verifier = encodeKey(crypto.getRandomValues(new Uint8Array(32)));
  return { verifier, challenge: encodeKey(sha256(new TextEncoder().encode(verifier))) };
}

interface TokenResponse {
  access_token: string;
  refresh_token: string;
  user: { id: string; email?: string | null };
}

function isTokenResponse(v: unknown): v is TokenResponse {
  const t = v as Partial<TokenResponse> | null;
  return (
    !!t &&
    typeof t.access_token === "string" &&
    typeof t.refresh_token === "string" &&
    typeof t.user?.id === "string"
  );
}

export interface AccountAuthOptions {
  config: AuthConfig;
  store: SecureStore;
  openAuthSession: OpenAuthSession;
  fetch?: typeof fetch;
  /** Unix seconds. */
  now?: () => number;
}

export class AccountAuth {
  readonly #config: AuthConfig;
  readonly #store: SecureStore;
  readonly #open: OpenAuthSession;
  readonly #fetch: typeof fetch;
  readonly #now: () => number;
  /** One renewal at a time: the refresh token rotates. */
  #renewing: Promise<string> | null = null;

  constructor(options: AccountAuthOptions) {
    this.#config = options.config;
    this.#store = options.store;
    this.#open = options.openAuthSession;
    this.#fetch = options.fetch ?? globalThis.fetch.bind(globalThis);
    this.#now = options.now ?? (() => Math.floor(Date.now() / 1000));
  }

  /** The signed-in account, from the store alone: works offline. */
  async current(): Promise<Account | null> {
    const s = await this.#store.accountSession();
    return s && { user_id: s.user_id, email: s.email };
  }

  /**
   * Run the whole sign-in: the provider page, its callback, the code
   * exchange. Keeps the session. `SignInCancelled` if the page was closed.
   */
  async signIn(): Promise<Account> {
    const { verifier, challenge } = pkce();
    const authorize = new URL(`${this.#config.url}/auth/v1/authorize`);
    authorize.searchParams.set("provider", "google");
    authorize.searchParams.set("redirect_to", AUTH_REDIRECT);
    authorize.searchParams.set("code_challenge", challenge);
    authorize.searchParams.set("code_challenge_method", "s256");

    const result = await this.#open(authorize.toString(), AUTH_REDIRECT);
    if (result.type !== "success" || !result.url) throw new SignInCancelled();
    const back = new URL(result.url);
    const error = back.searchParams.get("error_description") ?? back.searchParams.get("error");
    if (error) throw new Error(`Sign-in failed: ${error}`);
    const code = back.searchParams.get("code");
    if (!code) throw new Error("The sign-in page returned no code");

    const token = await this.#token("pkce", { auth_code: code, code_verifier: verifier });
    if (!token.ok) throw new Error(`The sign-in service refused the sign-in (${token.status})`);
    return this.#keep(token.body);
  }

  /**
   * A current access token: the kept one, or a renewed one when it expires
   * within a minute (or `force`, after the Cloud refused it). `unauthenticated`
   * when signed out or the renewal was refused; the session is then forgotten.
   */
  async accessToken(force = false): Promise<string> {
    const current = await this.#store.accountToken();
    const exp = current ? tokenExpiry(current) : null;
    if (current && !force && exp !== null && exp > this.#now() + REFRESH_MARGIN_S) return current;
    this.#renewing ??= this.#renew().finally(() => {
      this.#renewing = null;
    });
    return this.#renewing;
  }

  /** Forget the account here first (so it works offline), then revoke it upstream. */
  async signOut(): Promise<void> {
    const token = await this.#store.accountToken();
    await this.#store.setAccountSession(null);
    await this.#store.setAccountToken(null);
    if (!token) return;
    try {
      await this.#fetch(`${this.#config.url}/auth/v1/logout`, {
        method: "POST",
        headers: { apikey: this.#config.anonKey, authorization: `Bearer ${token}` },
      });
    } catch {
      // Revoked here regardless.
    }
  }

  async #renew(): Promise<string> {
    const session = await this.#store.accountSession();
    if (!session) throw signInRequired();
    const token = await this.#token("refresh_token", { refresh_token: session.refresh_token });
    if (!token.ok) {
      if (token.status >= 400 && token.status < 500) {
        await this.#store.setAccountSession(null);
        await this.#store.setAccountToken(null);
        throw signInRequired();
      }
      throw new ZwiimClientError("network", `The sign-in service is unavailable (${token.status})`);
    }
    // The same account, with the rotated tokens.
    await this.#keep(token.body, session);
    return token.body.access_token;
  }

  async #keep(token: TokenResponse, previous?: AccountSession): Promise<Account> {
    const account = { user_id: token.user.id, email: token.user.email ?? previous?.email ?? null };
    await this.#store.setAccountSession({ ...account, refresh_token: token.refresh_token });
    await this.#store.setAccountToken(token.access_token);
    return account;
  }

  async #token(
    grant: "pkce" | "refresh_token",
    body: Record<string, string>,
  ): Promise<{ ok: true; body: TokenResponse } | { ok: false; status: number }> {
    let res: Response;
    try {
      res = await this.#fetch(`${this.#config.url}/auth/v1/token?grant_type=${grant}`, {
        method: "POST",
        headers: { apikey: this.#config.anonKey, "content-type": "application/json" },
        body: JSON.stringify(body),
      });
    } catch {
      throw new ZwiimClientError("network", "Can't reach the sign-in service");
    }
    if (!res.ok) return { ok: false, status: res.status };
    let parsed: unknown;
    try {
      parsed = await res.json();
    } catch {
      parsed = null;
    }
    if (!isTokenResponse(parsed)) throw new ZwiimClientError("invalid_response", "Unexpected sign-in response");
    return { ok: true, body: parsed };
  }
}
