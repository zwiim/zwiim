// A scripted Supabase Auth (`/auth/v1/*`) and the platform's auth session,
// for the account tests. Access tokens are unsigned JWT-shaped strings: the
// app only reads their `exp`.

import { encodeKey } from "@zwiim/sdk";
import { sha256 } from "@noble/hashes/sha2.js";
import type { OpenAuthSession } from "../src/account/auth";

export const SUPABASE = "https://auth.test";
export const ANON_KEY = "anon-key";

const b64url = (s: string) => btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");

export function jwt(claims: Record<string, unknown>): string {
  return `${b64url('{"alg":"none"}')}.${b64url(JSON.stringify(claims))}.sig`;
}

export class FakeSupabase {
  /** Unix seconds; the tokens' `exp` is `now + ttl`. */
  now = 1_000_000;
  ttl = 3600;
  user = { id: "u1", email: "u1@example.com" };
  /** Answer every token request with this status instead. */
  failWith: number | null = null;
  readonly calls: { path: string; grant: string | null; body: Record<string, string> }[] = [];
  /** Issued access tokens, in order. */
  readonly issued: string[] = [];
  readonly revoked: string[] = [];
  /** code → its S256 challenge */
  readonly #codes = new Map<string, string>();
  /** live refresh tokens (rotated on use) */
  readonly #refresh = new Set<string>();
  #n = 0;
  /** The authorize URL the app opened last. */
  lastAuthorize: URL | null = null;

  #tokens() {
    const n = ++this.#n;
    const access = jwt({ sub: this.user.id, exp: this.now + this.ttl, n });
    const refresh = `refresh-${n}`;
    this.issued.push(access);
    this.#refresh.add(refresh);
    return { access_token: access, refresh_token: refresh, token_type: "bearer", user: this.user };
  }

  /** Revoke every refresh token (the account was signed out elsewhere). */
  revokeAll(): void {
    this.#refresh.clear();
  }

  /** The provider page: approves the sign-in and returns to the app with a code. */
  readonly openAuthSession: OpenAuthSession = async (url, redirect) => {
    const u = new URL(url);
    this.lastAuthorize = u;
    const code = `code-${++this.#n}`;
    this.#codes.set(code, u.searchParams.get("code_challenge") ?? "");
    return { type: "success", url: `${redirect}?code=${code}` };
  };

  readonly fetch = (async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
    const url = new URL(String(input));
    const body = init?.body ? (JSON.parse(String(init.body)) as Record<string, string>) : {};
    const grant = url.searchParams.get("grant_type");
    this.calls.push({ path: url.pathname, grant, body });
    const headers = new Headers(init?.headers);
    if (headers.get("apikey") !== ANON_KEY) return new Response("{}", { status: 401 });
    const json = (status: number, b: unknown) => new Response(JSON.stringify(b), { status });
    if (url.pathname === "/auth/v1/logout") {
      this.revoked.push(headers.get("authorization") ?? "");
      return new Response(null, { status: 204 });
    }
    if (url.pathname !== "/auth/v1/token") return json(404, {});
    if (this.failWith) return json(this.failWith, { error: "unavailable" });
    if (grant === "pkce") {
      const challenge = this.#codes.get(body.auth_code ?? "");
      this.#codes.delete(body.auth_code ?? "");
      const expected = encodeKey(sha256(new TextEncoder().encode(body.code_verifier ?? "")));
      if (!challenge || challenge !== expected) return json(400, { error: "invalid_grant" });
      return json(200, this.#tokens());
    }
    if (grant === "refresh_token") {
      if (!this.#refresh.delete(body.refresh_token ?? "")) return json(400, { error: "invalid_grant" });
      return json(200, this.#tokens());
    }
    return json(400, { error: "unsupported_grant_type" });
  }) as typeof fetch;
}
