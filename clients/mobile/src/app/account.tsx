// The signed-in account, for the screens under it, and signing out.

import { createContext, useContext } from "react";
import type { Account } from "../account/auth";

export interface AccountContext {
  account: Account;
  /** Sign out here: forget the account session, keep every pairing. */
  signOut(): Promise<void>;
  /** The account was signed out elsewhere (a refused renewal). */
  signedOut(): void;
}

export const AccountScope = createContext<AccountContext | null>(null);

export function useAccount(): AccountContext {
  const value = useContext(AccountScope);
  if (!value) throw new Error("useAccount outside AccountScope");
  return value;
}
