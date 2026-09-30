// The app's routes (DESIGN.md, Information Architecture): sign in, the
// account's Hosts, the Host shell with its tabs, a session or a new one, a worktree's
// changes, a file, a diff, a terminal or Browse Mac pushed over the shell,
// and pairing as a full-screen modal over either.

import type { NavigatorScreenParams } from "@react-navigation/native";
import type { NativeStackNavigationProp } from "@react-navigation/native-stack";
import { useNavigation } from "@react-navigation/native";
import type { HostEntry } from "../account/hosts";

export type HostTabs = {
  Sessions: undefined;
  Files: undefined;
  HostTab: undefined;
};

export type RootStack = {
  SignIn: undefined;
  Hosts: undefined;
  Host: NavigatorScreenParams<HostTabs> | undefined;
  /** `started`: this phone just started it, so the host may not list it yet. */
  Session: { session_id: string; started?: boolean };
  /** `workspace_id`: start there (a workspace just added from its Workspace sheet). */
  NewSession: { workspace_id?: string } | undefined;
  Changes: { workspace_id: string };
  File: { workspace_id: string; path: string };
  Diff: { workspace_id: string; path: string };
  TerminalView: { terminal_id: string };
  /** `then`: where to go once a folder is added (Files unless New session asked). */
  BrowseMac: { then?: "NewSession" } | undefined;
  /** `host`: the one chosen to pair; without it, any of `hosts` the scanned code is for. */
  Pair: { host?: HostEntry; hosts?: HostEntry[] };
};

export const useRootNavigation = () => useNavigation<NativeStackNavigationProp<RootStack>>();
