// The app's single instances: the secure store, the account, the account's
// Cloud, the host being controlled. Outside React state: screens call these, and secrets stay in the
// store until a call needs them.

import * as WebBrowser from "expo-web-browser";
import { AccountAuth } from "./account/auth";
import { config } from "./config";
import { CurrentHost } from "./host/current";
import { cloudFor, openHostConnection } from "./host/open";
import { lanDial, preferences, secureStore } from "./platform/native";

export const auth = config
  ? new AccountAuth({
      config: config.auth,
      store: secureStore,
      openAuthSession: (url, redirect) => WebBrowser.openAuthSessionAsync(url, redirect),
    })
  : null;

export const accessToken = async (): Promise<string> => {
  if (!auth) throw new Error("Sign-in is not set up in this build");
  return auth.accessToken();
};

export const cloud = config ? cloudFor(config.cloudUrl, accessToken) : null;

export const currentHost = new CurrentHost({
  store: secureStore,
  prefs: preferences,
  // The direct path dials where the host last said it listens: a host is
  // first reached through the Cloud, then directly on its network.
  open: (hostId, mode) =>
    openHostConnection(
      secureStore,
      { hostId, cloudUrl: config?.cloudUrl ?? null, lanEndpoint: null },
      { mode, dial: lanDial, accessToken },
    ),
});
