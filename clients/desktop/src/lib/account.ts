import { useCallback, useEffect, useRef, useState } from "react";
import { hostOwnedDetails } from "@zwiim/sdk";
import type { Account, AccountStatus, NativeBridge } from "@/lib/bridge";
import { errorMessage, useController, useHost, useOnline } from "@/lib/host-context";
import { rememberWelcomed } from "@/lib/prefs";
import { enableRemoteAccess } from "@/lib/remote";

/** How the signed-in account relates to the connected host. */
export type Ownership =
  | { kind: "local" }
  /** Reached through Zwiim Cloud: already the account's, never claimed from here. */
  | { kind: "remote" }
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
 *
 * `remote`: the host was reached through the account's Cloud, so it is the
 * account's already; `host.claim` is for the local host only.
 */
export function useAccount(
  bridge: NativeBridge,
  options: { remote?: boolean; onSignedOut?: () => void } = {},
): AccountState {
  const { remote = false, onSignedOut } = options;
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
  /** Just signed in on this computer: turn on remote access once the host is the account's. */
  const enableRemote = useRef(false);

  useEffect(() => {
    bridge.accountStatus().then(setStatus, (e) => {
      setStatus({ available: false, account: null, cloud_url: null });
      setError(errorMessage(e));
    });
  }, [bridge]);

  const account = status?.account ?? null;
  const owner = host?.owner ?? null;

  useEffect(() => {
    if (!online) claimed.current = null;
    if (remote || !account || !host || !online || owner) return;
    const key = `${host.host_id}:${account.user_id}`;
    if (claimed.current === key) return;
    claimed.current = key;
    // Re-claiming is idempotent for the owner, so a claim whose outcome was
    // lost is simply sent again on the next connection.
    controller.claimHost(account.user_id, account.email ?? undefined).catch((e) => {
      const d = hostOwnedDetails(e);
      if (d) setRefused({ hostId: host.host_id, userId: account.user_id, ownerEmail: d.owner_email });
    });
  }, [remote, account, host, owner, online, controller]);

  // Signed in here: the phone should find this Mac without a trip to Devices.
  // Best effort — Devices still offers Enable if this fails.
  useEffect(() => {
    const cloudUrl = status?.cloud_url;
    if (!enableRemote.current || remote || !account || owner?.user_id !== account.user_id || !online) return;
    enableRemote.current = false;
    if (!bridge.remote || !cloudUrl) return;
    const remoteBridge = bridge.remote;
    controller.client
      .request("cloud.status", {})
      .then((s) => (s.enrolled ? null : enableRemoteAccess(remoteBridge, cloudUrl, controller.client)))
      .catch(() => {});
  }, [remote, account, owner, online, status, bridge, controller]);

  let ownership: Ownership;
  if (remote) ownership = { kind: "remote" };
  else if (!account) ownership = { kind: "local" };
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
      enableRemote.current = true;
      setStatus((s) => ({ ...s, available: s?.available ?? true, account: next }));
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
      setStatus((s) => ({ ...s, available: s?.available ?? true, account: null }));
      onSignedOut?.();
    } catch (e) {
      setError(errorMessage(e));
    }
  }, [bridge, onSignedOut]);

  return { status, account, ownership, pending, error, signIn, cancelSignIn, signOut };
}
