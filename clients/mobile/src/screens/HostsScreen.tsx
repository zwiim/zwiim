// Hosts (DESIGN.md, Screen 2): the account's hosts from Zwiim Cloud, with
// presence and whether this phone is paired with each. Tapping a paired
// online host connects from the row, then opens the Host shell.

import { useCallback, useEffect, useState } from "react";
import { FlatList, RefreshControl, StyleSheet, View } from "react-native";
import { useFocusEffect } from "@react-navigation/native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { hostName, type HostEntry } from "../account/hosts";
import { useAccount } from "../app/account";
import { useAccountHosts, useCurrentHost } from "../app/hooks";
import { useRootNavigation } from "../app/navigation";
import { hostRowState } from "../host/status";
import { currentHost } from "../services";
import { Banner, Button, EmptyState, HostRow, SkeletonRows, T } from "../ui/components";
import { space, useTheme } from "../ui/tokens";

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

  useFocusEffect(
    useCallback(() => {
      void load();
    }, [load]),
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

  const now = Math.floor(Date.now() / 1000);
  return (
    <View style={[styles.page, { backgroundColor: theme.bg, paddingTop: insets.top }]}>
      <FlatList
        data={hosts ?? []}
        keyExtractor={(h) => h.host_id}
        refreshControl={<RefreshControl refreshing={loading && hosts !== null} onRefresh={() => void load()} tintColor={theme.mut} />}
        contentContainerStyle={{ paddingBottom: insets.bottom + space.xl }}
        ListHeaderComponent={
          <View style={styles.header}>
            <T variant="display" style={styles.title}>
              Hosts
            </T>
            {unreachable ? (
              <Banner text="Can't reach Zwiim Cloud." action={<Button kind="secondary" label="Retry" onPress={() => void load()} />} />
            ) : null}
            {hosts === null && !unreachable ? <SkeletonRows /> : null}
          </View>
        }
        ItemSeparatorComponent={() => <View style={[styles.divider, { backgroundColor: theme.line }]} />}
        ListEmptyComponent={
          hosts && !unreachable ? (
            <EmptyState
              glyph="⌂"
              title="No Hosts yet"
              body="Install Zwiim on your Mac and sign in with this account. It shows up here."
              action={<Button kind="secondary" label="Refresh" onPress={() => void load()} />}
            />
          ) : null
        }
        renderItem={({ item }) => {
          const row = hostRowState(item, current, modes[item.host_id], now);
          return (
            <HostRow
              testID={`host-${item.host_id}`}
              name={hostName(item)}
              meta={row.meta}
              dot={row.dot}
              onPress={() => open(item)}
              trailing={!item.paired ? <Button kind="secondary" label="Pair" onPress={() => open(item)} /> : null}
            />
          );
        }}
        ListFooterComponent={
          <View style={[styles.account, { borderTopColor: theme.line }]}>
            <T variant="caption" color={theme.mut}>
              Account
            </T>
            <T variant="mono" color={theme.mut} numberOfLines={1}>
              {account.email ?? account.user_id}
            </T>
            <View style={styles.signOut}>
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
        }
      />
    </View>
  );
}

const styles = StyleSheet.create({
  page: { flex: 1 },
  header: { paddingTop: space.xl, paddingBottom: space.md, gap: space.md },
  title: { paddingHorizontal: space.lg },
  divider: { height: StyleSheet.hairlineWidth, marginLeft: space.lg + 8 + space.md },
  account: {
    marginTop: space.xxl,
    marginHorizontal: space.lg,
    paddingTop: space.lg,
    gap: space.xs,
    borderTopWidth: StyleSheet.hairlineWidth,
  },
  signOut: { marginTop: space.sm, alignItems: "flex-start", marginLeft: -space.lg },
});
