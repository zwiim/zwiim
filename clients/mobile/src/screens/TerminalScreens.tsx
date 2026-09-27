// Terminals (DESIGN.md, Screen 7): a watching surface first, an input surface
// second. The Terminals segment of Files lists the selected worktree's
// terminals with a live preview; a terminal follows its output, takes a line
// or a key at a time, and can be killed. The phone never resizes the Host's
// PTY: it is shared with Desktop, so the view scrolls sideways instead.

import { useCallback, useEffect, useRef, useState } from "react";
import {
  FlatList,
  KeyboardAvoidingView,
  Platform,
  Pressable,
  StyleSheet,
  TextInput,
  View,
  type NativeScrollEvent,
  type NativeSyntheticEvent,
} from "react-native";
import { useIsFocused } from "@react-navigation/native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import type { NativeStackScreenProps } from "@react-navigation/native-stack";
import { isClientError, type TerminalMeta, type WorkspaceInfo } from "@zwiim/sdk";
import { useCurrentHost, useHostState, useTerminalScreen } from "../app/hooks";
import { useRootNavigation, type RootStack } from "../app/navigation";
import { contextLabel, contextText, terminalsIn, workspaceLabel } from "../host/workspace";
import { currentHost } from "../services";
import { keySequence, type TermLine } from "../terminal/screen";
import { Banner, Button, EmptyState, ScreenHeader, Sheet, StatusDot, T } from "../ui/components";
import { useCharWidth, WideList } from "../ui/code";
import { KeysRow, TERM_LINE_HEIGHT, TermRow, type TermKey } from "../ui/terminal";
import { MONO_MAX_SCALE, radius, space, type, useTheme } from "../ui/tokens";
import { useHostName } from "./HostFrame";

function terminalError(e: unknown, hostName: string, doing: string): string {
  if (isClientError(e, "network") || isClientError(e, "timeout")) return `Not connected to ${hostName}. Try again when it's back.`;
  if (isClientError(e, "unknown_outcome")) return "Not sure that went through. Refreshed from your Mac.";
  if (isClientError(e, "not_found")) return "This terminal has ended.";
  return `Couldn't ${doing}. Try again.`;
}

/** The Terminals segment of Files: the selected worktree's terminals, and New terminal. */
export function TerminalsSegment({ workspace }: { workspace: WorkspaceInfo }) {
  const nav = useRootNavigation();
  const name = useHostName();
  const { terminals, workspaces } = useHostState();
  const [spawning, setSpawning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const listed = terminalsIn(terminals, workspace.workspace_id);
  const worktree = contextLabel(workspaces, workspace.workspace_id)?.worktree ?? workspaceLabel(workspace);
  const spawn = async () => {
    setSpawning(true);
    setError(null);
    try {
      const terminal_id = await currentHost.spawnTerminal(workspace.workspace_id);
      nav.navigate("TerminalView", { terminal_id });
    } catch (e) {
      setError(terminalError(e, name, "start a terminal"));
    } finally {
      setSpawning(false);
    }
  };
  return (
    <View>
      {error ? <Banner text={error} /> : null}
      {listed.length === 0 ? (
        <EmptyState
          glyph=">_"
          title={`No terminals in ${worktree}`}
          body={`Start one to run commands on ${name}.`}
          action={<Button label="New terminal" loading={spawning} onPress={() => void spawn()} testID="terminal-new-empty" />}
        />
      ) : (
        <>
          <View style={styles.newRow}>
            <Button kind="secondary" label="New terminal" loading={spawning} onPress={() => void spawn()} testID="terminal-new" />
          </View>
          {listed.map((t) => (
            <TerminalRow key={t.terminal_id} meta={t} onPress={() => nav.navigate("TerminalView", { terminal_id: t.terminal_id })} />
          ))}
        </>
      )}
    </View>
  );
}

/** A terminal in the list: title, state and the last two lines it printed, live while the tab is shown. */
function TerminalRow({ meta, onPress }: { meta: TerminalMeta; onPress(): void }) {
  const theme = useTheme();
  const focused = useIsFocused();
  const screen = useTerminalScreen(focused && meta.alive ? meta.terminal_id : null);
  const preview = screen?.preview() ?? [];
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={`${meta.title}, ${meta.alive ? "running" : "exited"}`}
      onPress={onPress}
      style={({ pressed }) => [styles.row, pressed && { backgroundColor: theme.surface }]}
      testID={`terminal-${meta.terminal_id}`}
    >
      <View style={styles.rowHead}>
        <StatusDot state={meta.alive ? "done" : "idle"} />
        <T variant="mono" numberOfLines={1} style={styles.flex}>
          {meta.title}
        </T>
        <T variant="monoSm" color={theme.mut}>
          {meta.alive ? "running" : "exited"}
        </T>
      </View>
      {preview.length ? (
        <View style={[styles.preview, { backgroundColor: theme.code }]}>
          {preview.map((line, i) => (
            <T key={i} variant="monoSm" color={theme.mut} numberOfLines={1}>
              {line}
            </T>
          ))}
        </View>
      ) : null}
    </Pressable>
  );
}

export function TerminalScreenView({ route, navigation }: NativeStackScreenProps<RootStack, "TerminalView">) {
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  const { terminal_id } = route.params;
  const hostName = useHostName();
  const { view } = useCurrentHost();
  const { terminals, workspaces } = useHostState();
  const meta = terminals[terminal_id];
  const screen = useTerminalScreen(terminal_id);
  const back = () => (navigation.canGoBack() ? navigation.goBack() : navigation.navigate("Host"));
  const live = view?.status === "live";
  const alive = meta?.alive ?? false;
  const where = contextText(contextLabel(workspaces, meta?.workspace_id ?? null));
  const [confirming, setConfirming] = useState(false);
  const [killing, setKilling] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [line, setLine] = useState("");

  const send = useCallback(
    (data: string) => {
      setError(null);
      currentHost.writeTerminal(terminal_id, data).catch((e) => setError(terminalError(e, hostName, "send that")));
    },
    [terminal_id, hostName],
  );
  const key = (k: TermKey) => send(keySequence(k, screen?.applicationCursor ?? false));
  const kill = async () => {
    setKilling(true);
    setError(null);
    try {
      await currentHost.killTerminal(terminal_id);
      setConfirming(false);
    } catch (e) {
      setError(terminalError(e, hostName, "kill the terminal"));
      setConfirming(false);
    } finally {
      setKilling(false);
    }
  };

  const title = meta?.title ?? "Terminal";
  return (
    <View style={[styles.page, { backgroundColor: theme.bg, paddingTop: insets.top }]}>
      <ScreenHeader
        testID="terminal"
        title={title}
        subtitle={[where, meta ? (alive ? "running" : "exited") : null].filter(Boolean).join(" · ")}
        onBack={back}
        trailing={
          !meta ? null : alive ? (
            <Button kind="destructive" label="Kill" disabled={!live} onPress={() => setConfirming(true)} testID="terminal-kill" />
          ) : (
            <Button kind="secondary" label="Close" onPress={back} testID="terminal-close" />
          )
        }
      />
      {!meta && workspaces !== null ? (
        <EmptyState
          glyph=">_"
          title="This terminal is gone"
          body={`${hostName} no longer lists it.`}
          action={<Button kind="secondary" label="Back" onPress={back} />}
        />
      ) : (
        <KeyboardAvoidingView style={styles.page} behavior={Platform.OS === "ios" ? "padding" : "height"}>
          <View style={[styles.page, !live && styles.dim]}>
            <Output lines={screen?.lines() ?? []} />
          </View>
          {error ? <Banner text={error} /> : null}
          {alive ? (
            <View style={[styles.bar, { borderTopColor: theme.line, paddingBottom: Math.max(insets.bottom, space.sm) }]}>
              <KeysRow onKey={key} disabled={!live} />
              <TextInput
                value={line}
                onChangeText={setLine}
                placeholder="Run a command…"
                placeholderTextColor={theme.faint}
                editable={live}
                autoCapitalize="none"
                autoCorrect={false}
                spellCheck={false}
                returnKeyType="send"
                submitBehavior="submit"
                onSubmitEditing={() => {
                  send(`${line}\r`);
                  setLine("");
                }}
                maxFontSizeMultiplier={MONO_MAX_SCALE}
                style={[type.mono, styles.input, { color: theme.fg, backgroundColor: theme.surface }]}
                accessibilityLabel="Command"
                testID="terminal-input"
              />
            </View>
          ) : null}
        </KeyboardAvoidingView>
      )}
      <Sheet visible={confirming} onClose={() => setConfirming(false)}>
        <T variant="title">Kill {title}?</T>
        <T color={theme.mut}>Its process ends on {hostName}, for Desktop too.</T>
        <Button tall kind="destructive" label="Kill terminal" loading={killing} onPress={() => void kill()} testID="terminal-kill-confirm" />
        <Button tall kind="secondary" label="Cancel" onPress={() => setConfirming(false)} />
      </Sheet>
    </View>
  );
}

/** The output, following the tail until scrolled up; `↓ Live` resumes. */
function Output({ lines }: { lines: TermLine[] }) {
  const theme = useTheme();
  const list = useRef<FlatList<TermLine>>(null);
  const [following, setFollowing] = useState(true);
  const [cw, probe] = useCharWidth("term");
  const longest = lines.reduce((n, l) => Math.max(n, l.text.length), 0);

  useEffect(() => {
    if (following && lines.length) list.current?.scrollToEnd({ animated: false });
  }, [lines, following]);

  const onScroll = (e: NativeSyntheticEvent<NativeScrollEvent>) => {
    const { contentOffset, contentSize, layoutMeasurement } = e.nativeEvent;
    const atEnd = contentOffset.y + layoutMeasurement.height >= contentSize.height - TERM_LINE_HEIGHT * 2;
    if (atEnd !== following) setFollowing(atEnd);
  };

  return (
    <View style={[styles.page, { backgroundColor: theme.code }]} testID="terminal-output">
      {probe}
      <WideList
        ref={list}
        data={lines}
        width={Math.max(longest * cw + space.md * 2, 1)}
        keyExtractor={(l) => String(l.row)}
        renderItem={({ item }) => <TermRow line={item} />}
        getItemLayout={(_, i) => ({ length: TERM_LINE_HEIGHT, offset: TERM_LINE_HEIGHT * i + space.sm, index: i })}
        initialNumToRender={80}
        windowSize={7}
        contentContainerStyle={styles.output}
        onScrollBeginDrag={() => setFollowing(false)}
        onMomentumScrollEnd={onScroll}
        onScrollEndDrag={onScroll}
        keyboardShouldPersistTaps="handled"
      />
      {following ? null : (
        <Pressable
          accessibilityRole="button"
          accessibilityLabel="Follow live output"
          onPress={() => {
            setFollowing(true);
            list.current?.scrollToEnd({ animated: true });
          }}
          style={[styles.livePill, { backgroundColor: theme.raised }]}
          testID="terminal-live"
        >
          <T variant="monoSm" color={theme.accent}>
            ↓ Live
          </T>
        </Pressable>
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  page: { flex: 1 },
  flex: { flex: 1 },
  dim: { opacity: 0.6 },
  newRow: { flexDirection: "row", paddingHorizontal: space.lg, paddingVertical: space.sm },
  row: { minHeight: 52, paddingHorizontal: space.lg, paddingVertical: 11, gap: space.sm },
  rowHead: { flexDirection: "row", alignItems: "center", gap: space.sm },
  preview: { borderRadius: radius.sm, paddingHorizontal: space.sm, paddingVertical: space.xs },
  output: { paddingVertical: space.sm, paddingHorizontal: space.md },
  livePill: {
    position: "absolute",
    bottom: space.md,
    alignSelf: "center",
    height: 32,
    borderRadius: radius.full,
    paddingHorizontal: space.md,
    justifyContent: "center",
  },
  bar: { borderTopWidth: StyleSheet.hairlineWidth, paddingHorizontal: space.md, paddingTop: space.sm, gap: space.sm },
  input: { height: 44, borderRadius: radius.md, paddingHorizontal: space.md },
});
