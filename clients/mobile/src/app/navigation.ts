// The app's routes (DESIGN.md, Information Architecture): sign in, the
// account's Hosts, the Host shell with its tabs, a session, a worktree's
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
  Session: { session_id: string };
  Changes: { workspace_id: string };
  File: { workspace_id: string; path: string };
  Diff: { workspace_id: string; path: string };
  TerminalView: { terminal_id: string };
  BrowseMac: undefined;
  Pair: { host: HostEntry };
};

export const useRootNavigation = () => useNavigation<NativeStackNavigationProp<RootStack>>();
