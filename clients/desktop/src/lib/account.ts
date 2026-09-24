import { useCallback, useEffect, useRef, useState } from "react";
import { hostOwnedDetails } from "@zwiim/sdk";
import type { Account, AccountStatus, NativeBridge } from "@/lib/bridge";
import { errorMessage, useController, useHost, useOnline } from "@/lib/host-context";
import { rememberWelcomed } from "@/lib/prefs";

/** How the signed-in account relates to the connected host. */
export type Ownership =
  | { kind: "local" }
  | { kind: "claiming" }
  | { kind: "owner" }
  | { kind: "mismatch"; ownerEmail: string | null };

export interface AccountState {
  /** Null until the kept account has been read. */
  status: AccountStatus | null;
  account: Account | null;
  ownership: Ownership;
  /** Sign-in is waiting for the browser. */
  pending: boolean;
  error: string | null;
  signIn(): Promise<void>;
  cancelSignIn(): Promise<void>;
  signOut(): Promise<void>;
}

/**
 * The optional account. Signing in claims an unowned host for the account;
 * a host another account owns is never taken over — that shows as a
 * mismatch and local use carries on. Signing out only forgets the account
 * on this Desktop: the host, its agents, workspaces and sessions stay.
 */
export function useAccount(bridge: NativeBridge): AccountState {
  const controller = useController();
  const online = useOnline();
  const host = useHost((s) => s.host);
  const [status, setStatus] = useState<AccountStatus | null>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /** A claim the host refused: which host, for which account, owned by whom. */
  const [refused, setRefused] = useState<{ hostId: string; userId: string; ownerEmail: string | null } | null>(null);
  /** The host+account pair a claim was sent for on this connection. */
  const claimed = useRef<string | null>(null);

  useEffect(() => {
    bridge.accountStatus().then(setStatus, (e) => {
      setStatus({ available: false, account: null });
      setError(errorMessage(e));
    });
  }, [bridge]);

  const account = status?.account ?? null;
  const owner = host?.owner ?? null;

  useEffect(() => {
    if (!online) claimed.current = null;
    if (!account || !host || !online || owner) return;
    const key = `${host.host_id}:${account.user_id}`;
    if (claimed.current === key) return;
    claimed.current = key;
    // Re-claiming is idempotent for the owner, so a claim whose outcome was
    // lost is simply sent again on the next connection.
    controller.claimHost(account.user_id, account.email ?? undefined).catch((e) => {
      const d = hostOwnedDetails(e);
      if (d) setRefused({ hostId: host.host_id, userId: account.user_id, ownerEmail: d.owner_email });
    });
  }, [account, host, owner, online, controller]);

  let ownership: Ownership;
  if (!account) ownership = { kind: "local" };
  else if (owner) {
    ownership =
      owner.user_id === account.user_id ? { kind: "owner" } : { kind: "mismatch", ownerEmail: owner.email };
  } else if (refused && refused.hostId === host?.host_id && refused.userId === account.user_id) {
    ownership = { kind: "mismatch", ownerEmail: refused.ownerEmail };
  } else ownership = { kind: "claiming" };

  const signIn = useCallback(async () => {
    setPending(true);
    setError(null);
    try {
      const next = await bridge.signIn();
      claimed.current = null;
      setStatus((s) => ({ available: s?.available ?? true, account: next }));
      rememberWelcomed();
    } catch (e) {
      const msg = errorMessage(e);
      if (!/cancelled/i.test(msg)) setError(msg);
    } finally {
      setPending(false);
    }
  }, [bridge]);

  const cancelSignIn = useCallback(() => bridge.cancelSignIn(), [bridge]);

  const signOut = useCallback(async () => {
    setError(null);
    try {
      await bridge.signOut();
      claimed.current = null;
      setStatus((s) => ({ available: s?.available ?? true, account: null }));
    } catch (e) {
      setError(errorMessage(e));
    }
  }, [bridge]);

  return { status, account, ownership, pending, error, signIn, cancelSignIn, signOut };
}
