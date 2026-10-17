// The Host shell (DESIGN.md, Information Architecture): the current host's
// three tabs, each under the Host bar. Sessions lists the host's sessions and
// what they wait on; Files inspects the selected worktree and runs its
// terminals; the Host tab holds the connection, the host and the account. A pairing the host has
// dropped replaces the shell until paired again.

import { useEffect, useState } from "react";
import { Linking, Platform, Pressable, StyleSheet, View } from "react-native";
import { createBottomTabNavigator } from "@react-navigation/bottom-tabs";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import type { AgentAuthStatus, ConnectionMode } from "@zwiim/sdk";
import type { LucideIcon } from "lucide-react-native";
import { accountHosts, hostName, type HostEntry } from "../account/hosts";
import { useAccount } from "../app/account";
import { useCurrentHost, usePendingCount } from "../app/hooks";
import { useRootNavigation, type HostTabs } from "../app/navigation";
import {
  fetchMobileRelease,
  hostUpdateStep,
  optionalAppUpdate,
  resolveAppUpdate,
  type AppUpdate,
  type HostUpdateStep,
  type MobilePlatform,
  type MobileRelease,
} from "../host/release";
import { agentSignInRow, connectionText, hostProblem } from "../host/status";
import { appInfo } from "../platform/app-info";
import { secureStore } from "../platform/native";
import { releasesUrl } from "../config";
import { auth, cloud, currentHost } from "../services";
import { Button, PathPill, Sheet, StatusDot, T } from "../ui/components";
import { Folder, Icon, Laptop, MessagesSquare } from "../ui/icons";
import { fonts, radius, space, type, useTheme } from "../ui/tokens";
import { FilesTab } from "./FilesTab";
import { HostFrame } from "./HostFrame";
import { SessionsTab } from "./SessionsTab";

const Tabs = createBottomTabNavigator<HostTabs>();
const APP = appInfo(Platform.OS);
const PLATFORM: MobilePlatform = Platform.OS === "android" ? "android" : "ios";
const STORE = PLATFORM === "ios" ? "App Store" : "Google Play";

/** The latest release's store builds, read once per launch when first needed. */
let mobileRelease: Promise<MobileRelease | null> | null = null;
function latestMobileRelease(): Promise<MobileRelease | null> {
  mobileRelease ??= fetchMobileRelease(fetch, releasesUrl).then((r) => {
    if (!r) mobileRelease = null; // offline: ask again next time
    return r;
  });
  return mobileRelease;
}

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
  if (problem?.kind === "update") {
    const required = view?.compat?.status === "update_required" ? view.compat.requiredClientRevision : null;
    return (
      <UpdateScreen
        update={problem.update}
        title={problem.title}
        body={problem.body}
        hostName={hostName(host)}
        requiredRevision={required}
      />
    );
  }

  return (
    <Tabs.Navigator
      backBehavior="none"
      screenOptions={{
        headerShown: false,
        tabBarActiveTintColor: theme.fg,
        tabBarInactiveTintColor: theme.mut,
        tabBarStyle: { backgroundColor: theme.bg, borderTopColor: theme.line },
        tabBarLabelStyle: { fontFamily: fonts.sansMedium, fontSize: type.caption.fontSize },
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
          tabBarBadgeStyle: { backgroundColor: theme.need, color: theme.needInk, fontFamily: fonts.mono, fontSize: type.monoSm.fontSize },
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

/**
 * This app or the Host is too old for the other: nothing runs against the
 * Host. The SDK checks again by itself; Check again skips the wait. An old
 * app is sent to the store build that speaks what the Host needs (from the
 * release metadata); an old Host can be asked to update itself.
 */
function UpdateScreen({
  update,
  title,
  body,
  hostName: name,
  requiredRevision,
}: {
  update: "app" | "host";
  title: string;
  body: string;
  hostName: string;
  requiredRevision: number | null;
}) {
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  const nav = useRootNavigation();
  const [app, setApp] = useState<AppUpdate | null>(null);
  const [step, setStep] = useState<HostUpdateStep | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (update !== "app") return;
    let live = true;
    void latestMobileRelease().then((r) => live && setApp(resolveAppUpdate(r, PLATFORM, requiredRevision)));
    return () => {
      live = false;
    };
  }, [update, requiredRevision]);

  useEffect(() => {
    if (update !== "host") return;
    let live = true;
    const read = () =>
      currentHost.request("host.update.status", {}).then(
        (s) => live && setStep(hostUpdateStep(s, name)),
        () => live && setStep(hostUpdateStep(null, name)),
      );
    void read();
    const t = setInterval(() => void read(), 5000);
    return () => {
      live = false;
      clearInterval(t);
    };
  }, [update, name]);

  async function run(f: () => Promise<unknown>) {
    setBusy(true);
    setError(null);
    try {
      await f();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  const detail =
    update === "app"
      ? app?.kind === "store"
        ? `Zwiim ${app.version} on the ${STORE} works with ${name}.`
        : app?.kind === "pending"
          ? `The version ${name} needs isn't on the ${STORE} yet. Check again later.`
          : null
      : step?.kind === "apply"
        ? `Version ${step.version} is ready on ${name}. Restarting Zwiim there finishes the update; its sessions and workspaces are kept.`
        : step?.kind === "busy" || step?.kind === "manual"
          ? step.text
          : null;

  return (
    <View style={[styles.problem, { backgroundColor: theme.bg, paddingTop: insets.top + space.xxxl, paddingBottom: insets.bottom + space.xl }]}>
      <View style={styles.problemText}>
        <T variant="mono" color={theme.mut}>
          ↑
        </T>
        <T variant="title" testID="host-update-required">
          {title}
        </T>
        <T color={theme.mut}>{body}</T>
        {detail ? (
          <T color={theme.mut} testID="update-detail">
            {detail}
          </T>
        ) : null}
        {error ? <T color={theme.err}>{error}</T> : null}
      </View>
      <View style={styles.actions}>
        {update === "app" && app?.kind === "store" ? (
          <Button testID="open-store" tall label={`Open the ${STORE}`} onPress={() => void Linking.openURL(app.url)} />
        ) : null}
        {update === "host" && step?.kind === "apply" ? (
          <Button
            testID="host-apply-update"
            tall
            loading={busy}
            label={`Update ${name}`}
            onPress={() => void run(() => currentHost.request("host.update.apply", { mode: "when_idle" }))}
          />
        ) : null}
        {update === "host" && step?.kind === "check" ? (
          <Button
            testID="host-check-update"
            tall
            loading={busy}
            label={`Look for an update on ${name}`}
            onPress={() =>
              void run(async () => setStep(hostUpdateStep(await currentHost.request("host.update.check", {}), name)))
            }
          />
        ) : null}
        <Button testID="host-check-again" tall kind="secondary" label="Check again" onPress={() => currentHost.retry()} />
        <Button tall kind="ghost" label="Back to Hosts" onPress={() => nav.navigate("Hosts")} />
      </View>
    </View>
  );
}

/** A newer store build nobody requires: a quiet line in the Host tab. */
function useOptionalAppUpdate() {
  const [offer, setOffer] = useState<{ version: string; url: string } | null>(null);
  useEffect(() => {
    let live = true;
    void latestMobileRelease().then((r) => live && setOffer(optionalAppUpdate(r, PLATFORM, APP.version)));
    return () => {
      live = false;
    };
  }, []);
  return offer;
}

/** The Host tab (DESIGN.md, Screen 8): connection, host, account. */
function HostTab() {
  const theme = useTheme();
  const { host, view, hello } = useCurrentHost();
  const { account, signOut } = useAccount();
  const appUpdate = useOptionalAppUpdate();
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
          {view?.compat?.status === "limited" ? (
            <>
              <View style={[styles.divider, { backgroundColor: theme.line }]} />
              <View style={styles.groupRow}>
                <T color={theme.mut} testID="host-limited">
                  Some features need a newer Zwiim on {hostName(host)}.
                </T>
              </View>
            </>
          ) : null}
          <View style={[styles.divider, { backgroundColor: theme.line }]} />
          <View style={styles.groupRow}>
            <T variant="mono" color={theme.mut} testID="app-version">
              This phone: Zwiim {APP.version}
              {APP.build ? ` (${APP.build})` : ""}
            </T>
          </View>
          {appUpdate ? (
            <>
              <View style={[styles.divider, { backgroundColor: theme.line }]} />
              <Pressable style={styles.groupRow} onPress={() => void Linking.openURL(appUpdate.url)} testID="app-update">
                <T color={theme.accent}>
                  Zwiim {appUpdate.version} is on the {STORE}
                </T>
              </Pressable>
            </>
          ) : null}
        </View>

        <AgentsGroup live={view?.active === "lan" || view?.active === "relay"} />

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

/**
 * How the host's agents are signed in. Read only: keys and sign-in stay on
 * the Mac running the host, and no key is ever entered here.
 */
function AgentsGroup({ live }: { live: boolean }) {
  const theme = useTheme();
  const [statuses, setStatuses] = useState<AgentAuthStatus[] | null>(null);
  const [loading, setLoading] = useState(false);

  const read = () => {
    setLoading(true);
    return currentHost.request("agent.auth_status", {}).then(
      (s) => setStatuses(s),
      () => {},
    ).finally(() => setLoading(false));
  };

  useEffect(() => {
    if (live) void read();
  }, [live]);

  if (!statuses) return null;
  return (
    <>
      <T variant="caption" color={theme.mut}>
        Agents
      </T>
      <View style={[styles.group, { backgroundColor: theme.surface }]} testID="agents">
        {statuses.map((status) => {
          const row = agentSignInRow(status);
          return (
            <View key={status.agent}>
              <View style={styles.groupRow} testID={`agent-${status.agent}`}>
                <StatusDot state={row.dot} />
                <View style={styles.flex}>
                  <T>{row.name}</T>
                  {row.detail ? (
                    <T variant="mono" color={theme.mut} numberOfLines={1}>
                      {row.detail}
                    </T>
                  ) : null}
                </View>
                <T color={theme.mut}>{row.state}</T>
              </View>
              <View style={[styles.divider, { backgroundColor: theme.line }]} />
            </View>
          );
        })}
        <Pressable accessibilityRole="button" disabled={loading || !live} onPress={() => void read()} style={styles.groupRow} testID="agents-refresh">
          <T color={theme.mut} style={styles.flex}>
            Sign in on the Mac running Zwiim.
          </T>
          <T color={live ? theme.accent : theme.faint}>{loading ? "Refreshing…" : "Refresh"}</T>
        </Pressable>
      </View>
    </>
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
