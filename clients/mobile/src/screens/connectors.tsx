// The Host tab's Connectors group: each connector the host's agents can
// reach and how it is. Tap one to disconnect it; the rest is set up in Zwiim
// on the Mac.

import { useEffect, useState } from "react";
import { Pressable, StyleSheet, View } from "react-native";
import type { Connector } from "@zwiim/sdk";
import { connectorRow, disconnectConnector, loadConnectors } from "../host/connectors";
import { currentHost } from "../services";
import { Button, StatusDot, T } from "../ui/components";
import { radius, space, useTheme } from "../ui/tokens";

export function ConnectorsGroup({ host, live }: { host: string; live: boolean }) {
  const theme = useTheme();
  const [connectors, setConnectors] = useState<Connector[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  // The connector whose Disconnect confirm is open, and whether it's going.
  const [asking, setAsking] = useState<string | null>(null);
  const [disconnecting, setDisconnecting] = useState(false);

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

  const disconnect = async (id: string) => {
    setDisconnecting(true);
    const failed = await disconnectConnector(currentHost, id);
    setDisconnecting(false);
    setAsking(null);
    if (failed) return setError(failed);
    await read();
  };

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
            const gone = c.status === "revoking";
            return (
              <View key={c.connector_id}>
                <Pressable
                  accessibilityRole="button"
                  accessibilityHint="Shows Disconnect"
                  disabled={!live || gone || disconnecting}
                  onPress={() => setAsking(asking === c.connector_id ? null : c.connector_id)}
                  style={styles.row}
                  testID={`connector-${c.connector_id}`}
                >
                  <StatusDot state={row.dot} />
                  <View style={styles.flex}>
                    <T numberOfLines={1}>{row.name}</T>
                    <T variant="mono" color={theme.mut} numberOfLines={1}>
                      {row.detail}
                    </T>
                  </View>
                  <T color={theme.mut}>{row.state}</T>
                </Pressable>
                {asking === c.connector_id ? (
                  <View style={styles.confirm} testID="connector-confirm">
                    <T color={theme.mut}>
                      Disconnect {row.name}? Agents on {host} lose it and its key is deleted.
                    </T>
                    <View style={styles.buttons}>
                      <Button kind="ghost" label="Keep" disabled={disconnecting} onPress={() => setAsking(null)} />
                      <Button
                        kind="destructive"
                        label="Disconnect"
                        loading={disconnecting}
                        disabled={!live}
                        onPress={() => void disconnect(c.connector_id)}
                        testID="connector-disconnect"
                      />
                    </View>
                  </View>
                ) : null}
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
  confirm: { paddingHorizontal: space.md, paddingBottom: space.md, gap: space.sm },
  buttons: { flexDirection: "row", gap: space.sm, justifyContent: "flex-end" },
});
