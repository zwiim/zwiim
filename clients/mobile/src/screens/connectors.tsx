// The Host tab's Connectors group: each connector the host's agents can
// reach and how it is. Read only; it's set up in Zwiim on the Mac.

import { useEffect, useState } from "react";
import { Pressable, StyleSheet, View } from "react-native";
import type { Connector } from "@zwiim/sdk";
import { connectorRow, loadConnectors } from "../host/connectors";
import { currentHost } from "../services";
import { StatusDot, T } from "../ui/components";
import { radius, space, useTheme } from "../ui/tokens";

export function ConnectorsGroup({ host, live }: { host: string; live: boolean }) {
  const theme = useTheme();
  const [connectors, setConnectors] = useState<Connector[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const read = async () => {
    setLoading(true);
    const r = await loadConnectors(currentHost);
    setLoading(false);
    if (r === "outdated") return setConnectors(null);
    if ("error" in r) return setError(r.error);
    setError(null);
    setConnectors(r.connectors);
  };

  useEffect(() => {
    if (live) void read();
  }, [live]);

  // Nothing to show for a host without any, or from before them.
  if (!error && !connectors?.length) return null;
  return (
    <>
      <T variant="caption" color={theme.mut}>
        Connectors
      </T>
      <View style={[styles.group, { backgroundColor: theme.surface }]} testID="connectors">
        {error ? (
          <View style={styles.row}>
            <T color={theme.err} style={styles.flex} numberOfLines={2} testID="connectors-error">
              {error}
            </T>
          </View>
        ) : (
          connectors?.map((c) => {
            const row = connectorRow(c);
            return (
              <View key={c.connector_id} style={styles.row} testID={`connector-${c.connector_id}`}>
                <StatusDot state={row.dot} />
                <View style={styles.flex}>
                  <T numberOfLines={1}>{row.name}</T>
                  <T variant="mono" color={theme.mut} numberOfLines={1}>
                    {row.detail}
                  </T>
                </View>
                <T color={theme.mut}>{row.state}</T>
              </View>
            );
          })
        )}
        <View style={[styles.divider, { backgroundColor: theme.line }]} />
        <Pressable
          accessibilityRole="button"
          disabled={loading || !live}
          onPress={() => void read()}
          style={styles.row}
          testID="connectors-refresh"
        >
          <T color={theme.mut} style={styles.flex}>
            Set them up in Zwiim on {host}.
          </T>
          <T color={live ? theme.accent : theme.faint}>{loading ? "Refreshing…" : "Refresh"}</T>
        </Pressable>
      </View>
    </>
  );
}

const styles = StyleSheet.create({
  flex: { flex: 1 },
  group: { borderRadius: radius.lg, marginBottom: space.lg },
  row: { minHeight: 52, paddingHorizontal: space.md, flexDirection: "row", alignItems: "center", gap: space.sm },
  divider: { height: StyleSheet.hairlineWidth, marginLeft: space.md },
});
