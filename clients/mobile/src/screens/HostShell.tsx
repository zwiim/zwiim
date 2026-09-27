// The Host shell (DESIGN.md, Information Architecture): the current host's
// three tabs, each under the Host bar. Sessions lists the host's sessions and
// what they wait on; Files inspects the selected worktree and runs its
// terminals; the Host tab holds the connection, the host and the account. A pairing the host has
// dropped replaces the shell until paired again.

import { useState } from "react";
import { Pressable, StyleSheet, View } from "react-native";
import { createBottomTabNavigator } from "@react-navigation/bottom-tabs";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import type { ConnectionMode } from "@zwiim/sdk";
import type { LucideIcon } from "lucide-react-native";
import { accountHosts, hostName, type HostEntry } from "../account/hosts";
import { useAccount } from "../app/account";
import { useCurrentHost, usePendingCount } from "../app/hooks";
import { useRootNavigation, type HostTabs } from "../app/navigation";
import { connectionText, hostProblem } from "../host/status";
import { secureStore } from "../platform/native";
import { auth, cloud, currentHost } from "../services";
import { Button, PathPill, Sheet, T } from "../ui/components";
import { Folder, Icon, Laptop, MessagesSquare } from "../ui/icons";
import { fonts, radius, space, useTheme } from "../ui/tokens";
import { FilesTab } from "./FilesTab";
import { HostFrame } from "./HostFrame";
import { SessionsTab } from "./SessionsTab";

const Tabs = createBottomTabNavigator<HostTabs>();

const MODES: { mode: ConnectionMode; label: string; detail: string }[] = [
  { mode: "auto", label: "Automatic", detail: "Direct on the same network, Zwiim Cloud everywhere else." },
  { mode: "local", label: "Local only", detail: "Only when this phone is on the same network. Never uses the Cloud." },
  { mode: "cloud", label: "Cloud only", detail: "Always through Zwiim Cloud." },
];

const tabIcon = (glyph: LucideIcon) =>
  function TabIcon({ color }: { color: string }) {
    return <Icon icon={glyph} color={color} size={22} />;
  };
const ICONS = {
  Sessions: tabIcon(MessagesSquare),
  Files: tabIcon(Folder),
  Host: tabIcon(Laptop),
};

export function HostShell() {
  const theme = useTheme();
  const { host, view, openError } = useCurrentHost();
  const pending = usePendingCount();
  if (!host) return <View style={[styles.page, { backgroundColor: theme.bg }]} />;
  const problem = hostProblem(view, openError, hostName(host));
  if (problem?.kind === "screen") return <ProblemScreen title={problem.title} body={problem.body} />;

  return (
    <Tabs.Navigator
      backBehavior="none"
      screenOptions={{
        headerShown: false,
        tabBarActiveTintColor: theme.fg,
        tabBarInactiveTintColor: theme.faint,
        tabBarStyle: { backgroundColor: theme.bg, borderTopColor: theme.line },
        tabBarLabelStyle: { fontFamily: fonts.sansMedium, fontSize: 11 },
      }}
    >
      <Tabs.Screen
        name="Sessions"
        component={SessionsTab}
        options={{
          tabBarIcon: ICONS.Sessions,
          tabBarButtonTestID: "tab-sessions",
          // Requests waiting across the host.
          tabBarBadge: pending || undefined,
          tabBarBadgeStyle: { backgroundColor: theme.need, color: theme.needInk, fontFamily: fonts.mono, fontSize: 11 },
        }}
      />
      <Tabs.Screen name="Files" component={FilesTab} options={{ tabBarIcon: ICONS.Files, tabBarButtonTestID: "tab-files" }} />
      <Tabs.Screen
        name="HostTab"
        component={HostTab}
        options={{ title: "Host", tabBarIcon: ICONS.Host, tabBarButtonTestID: "tab-host" }}
      />
    </Tabs.Navigator>
  );
}

/** Pairing again, with the host's key as the account lists it now (it may have been reinstalled). */
function usePairAgain() {
  const nav = useRootNavigation();
  return async (host: HostEntry) => {
    let fresh = host;
    if (cloud && auth) {
      const listed = await accountHosts(cloud, secureStore, () => auth!.accessToken(true)).catch(() => []);
      fresh = listed.find((h) => h.host_id === host.host_id) ?? host;
    }
    nav.navigate("Pair", { host: fresh });
  };
}

/** Removed from the host, or it can't be verified: only pairing again helps. */
function ProblemScreen({ title, body }: { title: string; body: string }) {
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  const nav = useRootNavigation();
  const { host } = useCurrentHost();
  const pairAgain = usePairAgain();
  return (
    <View style={[styles.problem, { backgroundColor: theme.bg, paddingTop: insets.top + space.xxxl, paddingBottom: insets.bottom + space.xl }]}>
      <View style={styles.problemText}>
        <T variant="mono" color={theme.err}>
          ✕
        </T>
        <T variant="title" testID="host-problem">
          {title}
        </T>
        <T color={theme.mut}>{body}</T>
      </View>
      <View style={styles.actions}>
        <Button testID="host-pair-again" tall label="Pair again" onPress={() => host && void pairAgain(host)} />
        <Button tall kind="secondary" label="Back to Hosts" onPress={() => nav.navigate("Hosts")} />
      </View>
    </View>
  );
}

/** The Host tab (DESIGN.md, Screen 8): connection, host, account. */
function HostTab() {
  const theme = useTheme();
  const { host, view, hello } = useCurrentHost();
  const { account, signOut } = useAccount();
  const [choosing, setChoosing] = useState(false);
  const [signingOut, setSigningOut] = useState(false);
  if (!host) return null;
  const text = connectionText(view);
  const mode = MODES.find((m) => m.mode === (view?.mode ?? "auto"))!;
  return (
    <HostFrame title="Host" always>
      <View style={styles.groups}>
        <T variant="caption" color={theme.mut}>
          Connection
        </T>
        <View style={[styles.group, { backgroundColor: theme.surface }]}>
          <View style={styles.groupRow}>
            <T style={styles.flex} testID="host-status">
              {text.status}
            </T>
            <PathPill label={text.pill} />
          </View>
          <View style={[styles.divider, { backgroundColor: theme.line }]} />
          <Pressable accessibilityRole="button" onPress={() => setChoosing(true)} style={styles.groupRow} testID="host-mode">
            <T style={styles.flex}>Connection mode</T>
            <T color={theme.mut}>{mode.label}</T>
          </Pressable>
        </View>

        <T variant="caption" color={theme.mut}>
          Host
        </T>
        <View style={[styles.group, { backgroundColor: theme.surface }]}>
          <View style={styles.groupRow}>
            <T style={styles.flex}>{hostName(host)}</T>
          </View>
          <View style={[styles.divider, { backgroundColor: theme.line }]} />
          <View style={styles.groupRow}>
            <T variant="mono" color={theme.mut} selectable>
              {host.host_id}
            </T>
          </View>
          {hello ? (
            <>
              <View style={[styles.divider, { backgroundColor: theme.line }]} />
              <View style={styles.groupRow}>
                <T variant="mono" color={theme.mut} testID="host-version">
                  Zwiim {hello.version}
                </T>
              </View>
            </>
          ) : null}
        </View>

        <T variant="caption" color={theme.mut}>
          Account
        </T>
        <View style={[styles.group, { backgroundColor: theme.surface }]}>
          <View style={styles.groupRow}>
            <T variant="mono" color={theme.mut} numberOfLines={1} style={styles.flex}>
              {account.email ?? account.user_id}
            </T>
          </View>
          <View style={[styles.divider, { backgroundColor: theme.line }]} />
          <View style={styles.groupRow}>
            <Button
              testID="sign-out"
              kind="destructive"
              label="Sign out"
              loading={signingOut}
              onPress={() => {
                setSigningOut(true);
                void signOut();
              }}
            />
          </View>
        </View>
      </View>

      <Sheet visible={choosing} onClose={() => setChoosing(false)}>
        <T variant="title">Connection mode</T>
        {MODES.map((m) => (
          <Pressable
            key={m.mode}
            accessibilityRole="radio"
            accessibilityState={{ checked: m.mode === mode.mode }}
            testID={`mode-${m.mode}`}
            onPress={() => {
              void currentHost.setMode(m.mode);
              setChoosing(false);
            }}
            style={[styles.option, { backgroundColor: m.mode === mode.mode ? theme.surface : "transparent" }]}
          >
            <T variant="headline" color={m.mode === mode.mode ? theme.accent : theme.fg}>
              {m.label}
              {m.mode === "auto" ? "  (recommended)" : ""}
            </T>
            <T variant="callout" color={theme.mut}>
              {m.detail}
            </T>
          </Pressable>
        ))}
      </Sheet>
    </HostFrame>
  );
}

const styles = StyleSheet.create({
  page: { flex: 1 },
  flex: { flex: 1 },
  groups: { paddingHorizontal: space.lg, paddingTop: space.sm, gap: space.sm },
  group: { borderRadius: radius.lg, marginBottom: space.lg },
  groupRow: { minHeight: 52, paddingHorizontal: space.md, flexDirection: "row", alignItems: "center", gap: space.sm },
  divider: { height: StyleSheet.hairlineWidth, marginLeft: space.md },
  option: { borderRadius: radius.md, padding: space.md, gap: space.xxs },
  problem: { flex: 1, paddingHorizontal: space.lg, justifyContent: "space-between" },
  problemText: { gap: space.sm },
  actions: { gap: space.sm },
});
