// The app's routes (DESIGN.md, Information Architecture): sign in, the
// account's Hosts, the Host shell with its tabs, a session, a file, a diff or
// a terminal pushed over the shell, and pairing as a full-screen modal over
// either.

import type { NavigatorScreenParams } from "@react-navigation/native";
import type { NativeStackNavigationProp } from "@react-navigation/native-stack";
import { useNavigation } from "@react-navigation/native";
import type { HostEntry } from "../account/hosts";

export type HostTabs = {
  Sessions: undefined;
  Files: undefined;
  Terminal: undefined;
  HostTab: undefined;
};

export type RootStack = {
  SignIn: undefined;
  Hosts: undefined;
  Host: NavigatorScreenParams<HostTabs> | undefined;
  Session: { session_id: string };
  File: { workspace_id: string; path: string };
  Diff: { workspace_id: string; path: string };
  TerminalView: { terminal_id: string };
  Pair: { host: HostEntry };
};

export const useRootNavigation = () => useNavigation<NativeStackNavigationProp<RootStack>>();
