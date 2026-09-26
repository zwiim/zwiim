// The frame of every Host tab (DESIGN.md, Information Architecture): the
// Host bar with its switcher, the connection's banner, the tab's title, then
// the tab. Content shows only while live; while reconnecting it stays,
// dimmed, unless the host itself is offline.

import { useEffect, useState, type ReactNode } from "react";
import { ScrollView, StyleSheet, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { hostName } from "../account/hosts";
import { useAccount } from "../app/account";
import { useAccountHosts, useCurrentHost, useNow } from "../app/hooks";
import { useRootNavigation } from "../app/navigation";
import { connectionText, hostProblem, hostRowState } from "../host/status";
import { currentHost } from "../services";
import { Banner, Button, HostBar, HostRow, Sheet, SkeletonRows, T } from "../ui/components";
import { space, useTheme } from "../ui/tokens";

/**
 * A tab's frame: the Host bar, the connection's banner, then the tab. The
 * tab shows only while live (dimmed while reconnecting, hidden while the host
 * is offline) unless `always`. `accessory` sits in the title row.
 */
export function HostFrame({
  title,
  always,
  accessory,
  children,
}: {
  title: string;
  always?: boolean;
  accessory?: ReactNode;
  children: ReactNode;
}) {
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  const { host, view, openError } = useCurrentHost();
  const now = useNow(view?.status === "reconnecting" && view.retryAt !== null);
  const [switching, setSwitching] = useState(false);
  const { signOut } = useAccount();
  if (!host) return null;
  const name = hostName(host);
  const text = connectionText(view, now);
  const problem = hostProblem(view, openError, name);
  const banner = problem?.kind === "banner" ? problem : null;
  const status = view?.status;

  let content: ReactNode = null;
  if (always || status === "live") content = children;
  else if (status === "reconnecting" && view?.error?.code !== "host_offline") {
    // Last-known state, not actionable, under any banner but "offline".
    content = (
      <View style={styles.dim} pointerEvents="none">
        {children}
      </View>
    );
  } else if (banner) content = null;
  else if (!openError) content = <SkeletonRows />;

  return (
    <View style={[styles.page, { backgroundColor: theme.bg, paddingTop: insets.top }]}>
      <HostBar name={name} dot={text.dot} pill={text.pill} onPress={() => setSwitching(true)} />
      {banner ? (
        <Banner
          text={banner.text}
          action={
            banner.action === "switch" ? (
              <Button kind="secondary" label="Switch" onPress={() => void currentHost.setMode("auto")} />
            ) : banner.action === "sign_in" ? (
              <Button kind="secondary" label="Sign in" onPress={() => void signOut()} />
            ) : (
              <Button kind="secondary" label="Retry" onPress={() => currentHost.retry()} />
            )
          }
        />
      ) : null}
      <ScrollView contentContainerStyle={styles.scroll} keyboardShouldPersistTaps="handled">
        <View style={styles.titleRow}>
          <T variant="display" style={styles.title}>
            {title}
          </T>
          {accessory && status === "live" ? accessory : null}
        </View>
        {content}
      </ScrollView>
      <HostSwitcher visible={switching} onClose={() => setSwitching(false)} />
    </View>
  );
}

export function useHostName(): string {
  const { host } = useCurrentHost();
  return host ? hostName(host) : "";
}

/** The Host switcher (DESIGN.md, Key Flow 6): pick a host, the bar follows at once. */
function HostSwitcher({ visible, onClose }: { visible: boolean; onClose(): void }) {
  const theme = useTheme();
  const nav = useRootNavigation();
  const current = useCurrentHost();
  const { hosts, modes, unreachable, load } = useAccountHosts();
  useEffect(() => {
    if (visible) void load();
  }, [visible, load]);

  const now = Math.floor(Date.now() / 1000);
  return (
    <Sheet visible={visible} onClose={onClose}>
      <T variant="title">Hosts</T>
      {unreachable ? <T color={theme.mut}>Can't reach Zwiim Cloud.</T> : hosts === null ? <SkeletonRows count={2} /> : null}
      <View style={styles.switcher}>
        {(hosts ?? []).map((h) => {
          const row = hostRowState(h, current, modes[h.host_id], now);
          return (
            <HostRow
              key={h.host_id}
              testID={`switch-${h.host_id}`}
              name={hostName(h)}
              meta={h.host_id === current.host?.host_id ? `${row.meta} · this Host` : row.meta}
              dot={row.dot}
              onPress={() => {
                onClose();
                if (!h.paired) nav.navigate("Pair", { host: h });
                else void currentHost.select(h);
              }}
            />
          );
        })}
      </View>
      <Button
        tall
        kind="secondary"
        label="All Hosts"
        onPress={() => {
          onClose();
          nav.navigate("Hosts");
        }}
      />
    </Sheet>
  );
}

const styles = StyleSheet.create({
  page: { flex: 1 },
  scroll: { paddingBottom: space.xl },
  titleRow: { flexDirection: "row", alignItems: "center", gap: space.sm, paddingHorizontal: space.lg, paddingTop: space.lg, paddingBottom: space.sm },
  title: { flex: 1 },
  dim: { opacity: 0.6 },
  switcher: { marginHorizontal: -space.lg },
});
