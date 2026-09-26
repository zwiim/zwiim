// DEV ONLY: a temporary screen to drive the SDK on a phone. Pairs with a
// dev stack's host, connects in a chosen mode, and runs the checks. Opened
// with `zwiim://dev?provision=<url>` (add `&run=1` to run the checks), or
// with that link in `EXPO_PUBLIC_ZWIIM_DEV_LINK` when Metro starts.

import { useEffect, useState } from "react";
import { AppState, Linking, Pressable, ScrollView, StyleSheet, Text, TextInput, View } from "react-native";
import type { ConnectionMode } from "@zwiim/sdk";
import { errorInfo, type HostConnection, type HostConnectionView } from "../host/connection";
import { openHostConnection, type HostTarget } from "../host/open";
import { MemorySecureStore } from "../platform/memory-store";
import { lanDial } from "../platform/native";
import { runtimeProblem } from "../runtime";
import { runChecks, type CheckResult } from "./checks";
import { provisionFromDevStack } from "./provision";

// Outside React state: the store holds the device key and account token,
// and the connection holds the key for its handshakes.
const store = new MemorySecureStore();
const dev: { target: HostTarget | null; connection: HostConnection | null; provisionUrl: string | null } = {
  target: null,
  connection: null,
  provisionUrl: null,
};

const log = (line: string) => console.log(`[zwiim-dev] ${line}`);

const MODES: { mode: ConnectionMode; label: string }[] = [
  { mode: "auto", label: "Automatic" },
  { mode: "local", label: "Local only" },
  { mode: "cloud", label: "Cloud only" },
];

/** The provisioning URL in a `zwiim://dev?provision=…` link. */
export function devProvisionUrl(link: string): string | null {
  try {
    const url = new URL(link);
    return url.protocol === "zwiim:" && url.host === "dev" ? url.searchParams.get("provision") : null;
  } catch {
    return null;
  }
}

function stackUrl(provisionUrl: string, path: string): string {
  return provisionUrl.replace(/\/provision$/, `/${path}`);
}

async function post(url: string, body: unknown): Promise<void> {
  try {
    await fetch(url, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
  } catch {
    // The stack is optional for the screen.
  }
}

export default function DevScreen({ link: opened }: { link?: string | null } = {}) {
  const [problem] = useState(runtimeProblem);
  const [url, setUrl] = useState("");
  const [mode, setMode] = useState<ConnectionMode>("auto");
  const [paired, setPaired] = useState<string | null>(null);
  const [view, setView] = useState<HostConnectionView | null>(null);
  const [reply, setReply] = useState("");
  const [busy, setBusy] = useState(false);
  const [results, setResults] = useState<CheckResult[]>([]);

  const open = async (m: ConnectionMode): Promise<HostConnection> => {
    if (!dev.target) throw new Error("Not paired");
    return openHostConnection(store, dev.target, {
      mode: m,
      dial: lanDial,
      accessToken: async () => (await store.accountToken()) ?? "",
    });
  };

  const attach = async (m: ConnectionMode) => {
    dev.connection?.close();
    const c = await open(m);
    dev.connection = c;
    c.subscribe(setView);
    setView(c.view);
  };

  const pair = async (provisionUrl: string) => {
    setBusy(true);
    try {
      dev.target = await provisionFromDevStack(provisionUrl, store);
      dev.provisionUrl = provisionUrl;
      setPaired(dev.target.hostId);
      log(`paired with ${dev.target.hostId} (lan ${dev.target.lanEndpoint ? "known" : "none"})`);
      await attach(mode);
    } catch (e) {
      const err = errorInfo(e);
      log(`pairing failed: ${err.code}: ${err.message}`);
      setReply(`pairing failed: ${err.code}: ${err.message}`);
    } finally {
      setBusy(false);
    }
  };

  const check = async () => {
    const provisionUrl = dev.provisionUrl;
    dev.connection?.close();
    dev.connection = null;
    setView(null);
    setResults([]);
    setBusy(true);
    try {
      const all = await runChecks(
        open,
        (r) => {
          log(`${r.ok ? "ok  " : "FAIL"} ${r.step}: ${r.detail}`);
          setResults((prev) => [...prev, r]);
        },
        async (event) => {
          if (provisionUrl) await post(stackUrl(provisionUrl, "mark"), { event });
        },
      );
      const failed = all.filter((r) => !r.ok).length;
      log(`checks done: ${all.length - failed} passed, ${failed} failed`);
      if (provisionUrl) await post(stackUrl(provisionUrl, "report"), { results: all, runtime: problem });
    } catch (e) {
      const err = errorInfo(e);
      log(`checks stopped: ${err.code}: ${err.message}`);
    } finally {
      setBusy(false);
    }
    await attach(mode);
  };

  // Deep link: zwiim://dev?provision=<url>[&run=1]
  useEffect(() => {
    /** Whether `link` was a dev link. */
    const handle = async (link: string | null): Promise<boolean> => {
      const provisionUrl = link ? devProvisionUrl(link) : null;
      if (!link || !provisionUrl) return false;
      setUrl(provisionUrl);
      await pair(provisionUrl);
      if (new URL(link).searchParams.get("run") === "1" && dev.target) await check();
      return true;
    };
    // Or set when the bundle was built: EXPO_PUBLIC_ZWIIM_DEV_LINK=<the same link>.
    // (On Android a dev build is launched with its own link, so that is no dev link.)
    // A link that arrived before this screen opened comes as `opened` (read on mount only).
    void (opened ? Promise.resolve(opened) : Linking.getInitialURL()).then(
      async (link) => (await handle(link)) || handle(process.env.EXPO_PUBLIC_ZWIIM_DEV_LINK ?? null),
    );
    const sub = Linking.addEventListener("url", ({ url: link }) => void handle(link));
    // Back in the foreground: reconnect now rather than after the backoff.
    const app = AppState.addEventListener("change", (s) => s === "active" && dev.connection?.resume());
    return () => {
      sub.remove();
      app.remove();
      dev.connection?.close();
      dev.connection = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const chooseMode = (m: ConnectionMode) => {
    setMode(m);
    dev.connection?.setMode(m);
  };

  const call = async (op: "host.hello" | "workspace.list") => {
    const c = dev.connection;
    if (!c) return;
    try {
      const result = await c.request(op, {});
      setReply(`${op} → ${JSON.stringify(result, null, 2)}`);
    } catch (e) {
      const err = errorInfo(e);
      setReply(`${op} failed: ${err.code}: ${err.message}`);
    }
  };

  return (
    <ScrollView contentContainerStyle={styles.page}>
      <Text style={styles.title}>Zwiim · dev</Text>
      <Row label="SDK" value={problem ? `unavailable: ${problem}` : "ready"} />
      <Row label="Host" value={view?.hostId ?? paired ?? "not paired"} />
      <Row label="Mode" value={MODES.find((m) => m.mode === mode)?.label ?? mode} />
      <Row label="State" value={view ? `${view.status} · ${view.active}` : "—"} />
      <Row label="Events" value={String(view?.events ?? 0)} />
      {view?.error ? <Row label="Error" value={`${view.error.code}: ${view.error.message}`} /> : null}

      <TextInput
        style={styles.input}
        value={url}
        onChangeText={setUrl}
        placeholder="Dev stack provisioning URL"
        autoCapitalize="none"
        autoCorrect={false}
      />
      <View style={styles.row}>
        <Button label="Pair" onPress={() => void pair(url)} disabled={busy || !url} />
        <Button label="Run checks" onPress={() => void check()} disabled={busy || !paired} />
      </View>
      <View style={styles.row}>
        {MODES.map((m) => (
          <Button key={m.mode} label={m.label} onPress={() => chooseMode(m.mode)} selected={mode === m.mode} />
        ))}
      </View>
      <View style={styles.row}>
        <Button label="Connect" onPress={() => dev.connection?.connect()} disabled={busy || !paired} />
        <Button label="Disconnect" onPress={() => dev.connection?.disconnect()} disabled={busy || !paired} />
      </View>
      <View style={styles.row}>
        <Button label="host.hello" onPress={() => void call("host.hello")} disabled={view?.status !== "live"} />
        <Button label="workspace.list" onPress={() => void call("workspace.list")} disabled={view?.status !== "live"} />
      </View>
      {reply ? <Text style={styles.mono}>{reply}</Text> : null}
      {results.map((r, i) => (
        <Text key={i} style={[styles.mono, !r.ok && styles.fail]}>
          {r.ok ? "✓" : "✗"} {r.step}: {r.detail}
        </Text>
      ))}
    </ScrollView>
  );
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <View style={styles.row}>
      <Text style={styles.label}>{label}</Text>
      <Text style={styles.value} testID={`dev-${label.toLowerCase()}`}>
        {value}
      </Text>
    </View>
  );
}

function Button(props: { label: string; onPress(): void; disabled?: boolean; selected?: boolean }) {
  return (
    <Pressable
      onPress={props.onPress}
      disabled={props.disabled}
      style={[styles.button, props.selected && styles.selected, props.disabled && styles.disabled]}
    >
      <Text style={[styles.buttonText, props.selected && styles.selectedText]}>{props.label}</Text>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  page: { padding: 20, paddingTop: 64, gap: 10 },
  title: { fontSize: 22, fontWeight: "600", marginBottom: 8 },
  row: { flexDirection: "row", flexWrap: "wrap", gap: 8, alignItems: "center" },
  label: { width: 64, color: "#666" },
  value: { flex: 1, fontWeight: "500" },
  input: { borderWidth: 1, borderColor: "#ccc", borderRadius: 8, padding: 10, marginTop: 8 },
  button: { borderWidth: 1, borderColor: "#999", borderRadius: 8, paddingVertical: 8, paddingHorizontal: 12 },
  selected: { backgroundColor: "#222", borderColor: "#222" },
  selectedText: { color: "#fff" },
  disabled: { opacity: 0.4 },
  buttonText: { fontSize: 14 },
  mono: { fontFamily: "Menlo", fontSize: 12 },
  fail: { color: "#b00020" },
});
