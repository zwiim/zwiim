// Hosts (DESIGN.md, Screen 2): the account's hosts from Zwiim Cloud, with
// presence and whether this phone is paired with each. Tapping a paired
// online host connects from the row, then opens the Host shell. Until this
// phone is paired with one, it is Connect your Mac: a checklist that ticks
// itself as the Mac turns up, with Scan pairing code always in reach.

import { useCallback, useEffect, useState } from "react";
import { RefreshControl, ScrollView, StyleSheet, View } from "react-native";
import { useFocusEffect } from "@react-navigation/native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { hostName, macToPair, type HostEntry } from "../account/hosts";
import { useAccount } from "../app/account";
import { useAccountHosts, useCurrentHost } from "../app/hooks";
import { useRootNavigation } from "../app/navigation";
import { hostRowState } from "../host/status";
import { currentHost } from "../services";
import { Banner, Button, HostRow, SkeletonRows, T } from "../ui/components";
import { Check, Icon, ScanLine } from "../ui/icons";
import { fonts, radius, space, useTheme } from "../ui/tokens";

/** While setting up, how often to look for the Mac. */
const SETUP_POLL_MS = 4000;

export function HostsScreen() {
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  const nav = useRootNavigation();
  const { account, signOut } = useAccount();
  const { hosts, modes, loading, unreachable, load } = useAccountHosts();
  const current = useCurrentHost();
  // A host being connected from its row; the shell opens once that settles.
  const [waiting, setWaiting] = useState<string | null>(null);
  const [signingOut, setSigningOut] = useState(false);

  const mac = hosts && !unreachable ? macToPair(hosts) : undefined;
  const settingUp = mac !== undefined;

  useFocusEffect(
    useCallback(() => {
      void load();
      if (!settingUp) return;
      const t = setInterval(() => void load(), SETUP_POLL_MS);
      return () => clearInterval(t);
    }, [load, settingUp]),
  );

  useEffect(() => {
    if (!waiting) return;
    if (current.host?.host_id !== waiting) return setWaiting(null);
    const status = current.view?.status;
    const settled = current.openError || (status && status !== "disconnected" && status !== "connecting" && status !== "handshaking");
    if (!settled) return;
    setWaiting(null);
    nav.navigate("Host");
  }, [waiting, current, nav]);

  const open = (host: HostEntry) => {
    if (!host.paired) return nav.navigate("Pair", { host });
    void currentHost.select(host);
    // An offline host opens straight into its offline state.
    if (host.online) setWaiting(host.host_id);
    else nav.navigate("Host");
  };

  if (settingUp) {
    return (
      <ConnectYourMac
        email={account.email ?? account.user_id}
        mac={mac}
        onScan={() => nav.navigate("Pair", { hosts: hosts ?? [] })}
        signingOut={signingOut}
        onSignOut={() => {
          setSigningOut(true);
          void signOut();
        }}
      />
    );
  }

  const now = Math.floor(Date.now() / 1000);
  return (
    <View style={[styles.page, { backgroundColor: theme.bg, paddingTop: insets.top }]}>
      <ScrollView
        refreshControl={<RefreshControl refreshing={loading && hosts !== null} onRefresh={() => void load()} tintColor={theme.mut} />}
        contentContainerStyle={[styles.list, { paddingBottom: insets.bottom + space.xl }]}
      >
        <T variant="display">Hosts</T>
        {unreachable ? (
          <Banner text="Can't reach Zwiim Cloud." action={<Button kind="secondary" label="Retry" onPress={() => void load()} />} />
        ) : null}
        {hosts === null && !unreachable ? <SkeletonRows /> : null}
        {hosts?.length ? (
          <View style={[styles.card, { backgroundColor: theme.surface }]}>
            {hosts.map((item, i) => {
              const row = hostRowState(item, current, modes[item.host_id], now);
              return (
                <View key={item.host_id} style={i > 0 && [styles.divided, { borderTopColor: theme.line }]}>
                  <HostRow
                    testID={`host-${item.host_id}`}
                    name={hostName(item)}
                    meta={row.meta}
                    dot={row.dot}
                    onPress={() => open(item)}
                    trailing={!item.paired ? <Button kind="secondary" label="Pair" onPress={() => open(item)} /> : undefined}
                  />
                </View>
              );
            })}
          </View>
        ) : null}
        <View style={[styles.card, styles.account, { backgroundColor: theme.surface }]}>
          <View style={styles.flex}>
            <T variant="caption" color={theme.mut}>
              Account
            </T>
            <T variant="mono" numberOfLines={1}>
              {account.email ?? account.user_id}
            </T>
          </View>
          <Button
            testID="sign-out"
            kind="secondary"
            label="Sign out"
            loading={signingOut}
            onPress={() => {
              setSigningOut(true);
              void signOut();
            }}
          />
        </View>
      </ScrollView>
    </View>
  );
}

type Mark = "done" | "now" | "todo";

/** Connect your Mac: signed in ✓, the Mac, the scan, each ticking as it happens. */
function ConnectYourMac({
  email,
  mac,
  onScan,
  signingOut,
  onSignOut,
}: {
  email: string;
  /** The Mac to pair, or null while none has turned on remote access. */
  mac: HostEntry | null;
  onScan(): void;
  signingOut: boolean;
  onSignOut(): void;
}) {
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  const macStep: { mark: Mark; title: string; meta: string } = !mac
    ? {
        mark: "now",
        title: "Zwiim on your Mac",
        meta: "waiting for your Mac…\nsign in with the same account, then\nSettings → Devices → Remote access: On",
      }
    : mac.online
      ? { mark: "done", title: `${hostName(mac)} is online`, meta: "in your account" }
      : { mark: "now", title: `${hostName(mac)} is offline`, meta: "open Zwiim on your Mac" };
  return (
    <View
      style={[
        styles.page,
        { backgroundColor: theme.bg, paddingTop: insets.top, paddingBottom: insets.bottom + space.xl },
      ]}
    >
      <ScrollView contentContainerStyle={styles.setup}>
        <T variant="display" testID="connect-your-mac">
          Connect your Mac
        </T>
        <View style={[styles.checklist, { backgroundColor: theme.surface }]}>
          <SetupItem first mark="done" title="Signed in on this phone" meta={email} />
          <SetupItem {...macStep} />
          <SetupItem mark={mac?.online ? "now" : "todo"} title="Scan the pairing code" meta="Devices → Pair a device" />
        </View>
        {!mac ? (
          <T variant="callout" color={theme.mut}>
            Your Mac shows up here by itself.
          </T>
        ) : null}
      </ScrollView>
      <View style={styles.setupActions}>
        <Button testID="scan-pairing-code" tall icon={ScanLine} label="Scan pairing code" onPress={onScan} />
        <Button testID="sign-out" kind="ghost" label="Sign out" loading={signingOut} onPress={onSignOut} />
      </View>
    </View>
  );
}

function SetupItem({ first, mark, title, meta }: { first?: boolean; mark: Mark; title: string; meta: string }) {
  const theme = useTheme();
  return (
    <View style={[styles.item, { borderTopColor: theme.line }, first && styles.firstItem]}>
      {mark === "done" ? (
        <View style={[styles.mark, { backgroundColor: `${theme.done}2E` }]}>
          <Icon icon={Check} color={theme.done} size={13} />
        </View>
      ) : (
        <View style={[styles.mark, styles.ring, { borderColor: mark === "now" ? theme.accent : theme.faint }]}>
          {mark === "now" ? <View style={[styles.pip, { backgroundColor: theme.accent }]} /> : null}
        </View>
      )}
      <View style={styles.itemText}>
        <T style={styles.itemTitle} color={mark === "todo" ? theme.mut : theme.fg}>
          {title}
        </T>
        <T variant="monoSm" color={theme.mut}>
          {meta}
        </T>
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  page: { flex: 1 },
  setup: { paddingHorizontal: space.lg, paddingTop: space.xl, gap: space.md },
  checklist: { borderRadius: radius.lg, overflow: "hidden", marginTop: space.sm },
  item: {
    flexDirection: "row",
    gap: space.md,
    paddingHorizontal: 14,
    paddingVertical: 13,
    borderTopWidth: StyleSheet.hairlineWidth,
  },
  mark: {
    width: 22,
    height: 22,
    borderRadius: radius.full,
    alignItems: "center",
    justifyContent: "center",
    marginTop: 1,
  },
  firstItem: { borderTopWidth: 0 },
  ring: { borderWidth: 1.5 },
  pip: { width: 8, height: 8, borderRadius: radius.full },
  itemText: { flex: 1, gap: 3 },
  itemTitle: { fontFamily: fonts.sansMedium, lineHeight: 20 },
  setupActions: { paddingHorizontal: space.lg, gap: space.sm },
  list: { flexGrow: 1, paddingHorizontal: space.lg, paddingTop: space.xl, gap: space.md },
  card: { borderRadius: radius.lg, overflow: "hidden" },
  divided: { borderTopWidth: StyleSheet.hairlineWidth },
  account: {
    marginTop: "auto",
    flexDirection: "row",
    alignItems: "center",
    gap: space.md,
    paddingVertical: space.md,
    paddingHorizontal: space.lg,
  },
  flex: { flex: 1, gap: space.xxs },
});
