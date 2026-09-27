// Terminals (DESIGN.md, Screen 7): a watching surface first, an input surface
// second. The dock under Files says what runs in the selected worktree and
// opens a sheet listing its terminals with a live preview; a terminal follows
// its output, takes a line or a key at a time, and can be killed. The phone never resizes the Host's
// PTY: it is shared with Desktop, so the view scrolls sideways instead.

import { useCallback, useEffect, useRef, useState } from "react";
import {
  FlatList,
  KeyboardAvoidingView,
  Platform,
  Pressable,
  ScrollView,
  StyleSheet,
  TextInput,
  View,
  type NativeScrollEvent,
  type NativeSyntheticEvent,
} from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import type { NativeStackScreenProps } from "@react-navigation/native-stack";
import { isClientError, type TerminalMeta, type WorkspaceInfo } from "@zwiim/sdk";
import { useCurrentHost, useHostState, useTerminalScreen } from "../app/hooks";
import { useRootNavigation, type RootStack } from "../app/navigation";
import { contextLabel, contextText, terminalsIn, terminalSummary, workspaceLabel } from "../host/workspace";
import { currentHost } from "../services";
import { keySequence, type TermLine } from "../terminal/screen";
import { Banner, Button, EmptyState, ScreenHeader, Sheet, T } from "../ui/components";
import { useCharWidth, WideList } from "../ui/code";
import { ArrowUp, ChevronRight, ChevronUp, Icon } from "../ui/icons";
import { KeysRow, TERM_LINE_HEIGHT, TermRow, type TermKey } from "../ui/terminal";
import { fonts, MONO_MAX_SCALE, radius, space, type, useTheme } from "../ui/tokens";
import { useHostName } from "./HostFrame";

function terminalError(e: unknown, hostName: string, doing: string): string {
  if (isClientError(e, "network") || isClientError(e, "timeout")) return `Not connected to ${hostName}. Try again when it's back.`;
  if (isClientError(e, "unknown_outcome")) return "Not sure that went through. Refreshed from your Mac.";
  if (isClientError(e, "not_found")) return "This terminal has ended.";
  return `Couldn't ${doing}. Try again.`;
}

/**
 * The Terminals dock above the tab bar: what runs in the selected worktree,
 * a dot per terminal. It opens the Terminals sheet.
 */
export function TerminalsDock({ workspace }: { workspace: WorkspaceInfo }) {
  const theme = useTheme();
  const { terminals, workspaces } = useHostState();
  const [open, setOpen] = useState(false);
  const listed = terminalsIn(terminals, workspace.workspace_id);
  const scope = contextLabel(workspaces, workspace.workspace_id)?.worktree ?? workspaceLabel(workspace);
  const summary = terminalSummary(listed, scope);
  return (
    <>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={`Terminals, ${summary}`}
        onPress={() => setOpen(true)}
        style={({ pressed }) => [styles.dock, { backgroundColor: pressed ? theme.raised : theme.surface, borderTopColor: theme.line }]}
        testID="terminals-dock"
      >
        <T variant="mono" color={theme.mut}>
          &gt;_
        </T>
        <T style={styles.dockTitle}>Terminals</T>
        <T variant="monoSm" color={theme.mut} numberOfLines={1} style={styles.flex}>
          {summary}
        </T>
        <View style={styles.dots}>
          {listed.map((t) => (
            <View key={t.terminal_id} style={[styles.dockDot, { backgroundColor: t.alive ? theme.accent : theme.faint }]} />
          ))}
        </View>
        <Icon icon={ChevronUp} color={theme.faint} size={16} />
      </Pressable>
      <TerminalsSheet workspace={workspace} scope={scope} visible={open} onClose={() => setOpen(false)} />
    </>
  );
}

/** The selected worktree's terminals with a live preview, and New terminal. */
function TerminalsSheet({
  workspace,
  scope,
  visible,
  onClose,
}: {
  workspace: WorkspaceInfo;
  scope: string;
  visible: boolean;
  onClose(): void;
}) {
  const theme = useTheme();
  const nav = useRootNavigation();
  const name = useHostName();
  const { terminals, workspaces } = useHostState();
  const [spawning, setSpawning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const listed = terminalsIn(terminals, workspace.workspace_id);
  const where = contextText(contextLabel(workspaces, workspace.workspace_id)) ?? workspaceLabel(workspace);
  const open = (terminal_id: string) => {
    onClose();
    nav.navigate("TerminalView", { terminal_id });
  };
  const spawn = async () => {
    setSpawning(true);
    setError(null);
    try {
      open(await currentHost.spawnTerminal(workspace.workspace_id));
    } catch (e) {
      setError(terminalError(e, name, "start a terminal"));
    } finally {
      setSpawning(false);
    }
  };
  return (
    <Sheet visible={visible} onClose={onClose}>
      <View style={styles.sheetHead}>
        <T variant="title">Terminals</T>
        <T variant="monoSm" color={theme.mut} numberOfLines={1}>
          {where} · on {name}
        </T>
      </View>
      <ScrollView style={[styles.sheetList, { borderTopColor: theme.line }]}>
        {visible
          ? listed.map((t) => <TerminalRow key={t.terminal_id} meta={t} onPress={() => open(t.terminal_id)} />)
          : null}
        {listed.length === 0 ? (
          <View style={styles.sheetEmpty}>
            <T variant="mono" color={theme.faint}>
              &gt;_
            </T>
            <T variant="title">No terminals in {scope}</T>
            <T color={theme.mut}>Start one to run commands on {name}.</T>
          </View>
        ) : null}
      </ScrollView>
      {error ? (
        <T color={theme.err} testID="terminal-error">
          {error}
        </T>
      ) : null}
      <Button tall label="New terminal" loading={spawning} onPress={() => void spawn()} testID="terminal-new" />
    </Sheet>
  );
}

/** A terminal in the sheet: title, state and the last two lines it printed, live while shown. */
function TerminalRow({ meta, onPress }: { meta: TerminalMeta; onPress(): void }) {
  const theme = useTheme();
  const screen = useTerminalScreen(meta.alive ? meta.terminal_id : null);
  const preview = screen?.preview() ?? [];
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={`${meta.title ?? "Terminal"}, ${meta.alive ? "running" : "exited"}`}
      onPress={onPress}
      style={({ pressed }) => [styles.row, { borderBottomColor: theme.line }, pressed && { backgroundColor: theme.surface }]}
      testID={`terminal-${meta.terminal_id}`}
    >
      <View style={[styles.rowDot, { backgroundColor: meta.alive ? theme.accent : theme.faint }]} />
      <View style={styles.rowText}>
        <View style={styles.rowHead}>
          <T variant="mono" numberOfLines={1} style={styles.rowTitle}>
            {meta.title ?? "terminal"}
          </T>
          <T variant="monoSm" color={meta.alive ? theme.accent : theme.mut}>
            {meta.alive ? "running" : "exited"}
          </T>
        </View>
        {preview.map((line, i) => (
          <T key={i} variant="monoSm" color={theme.mut} numberOfLines={1}>
            {line}
          </T>
        ))}
      </View>
      <Icon icon={ChevronRight} color={theme.faint} size={18} />
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
  const submit = () => {
    send(`${line}\r`);
    setLine("");
  };
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
            <Button kind="destructive" label="Close" onPress={back} testID="terminal-close" />
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
            <View style={[styles.bar, { backgroundColor: theme.bg, borderTopColor: theme.line, paddingBottom: Math.max(insets.bottom, space.sm) }]}>
              <View style={styles.inputRow}>
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
                  onSubmitEditing={submit}
                  maxFontSizeMultiplier={MONO_MAX_SCALE}
                  style={[type.mono, styles.input, { color: theme.fg, backgroundColor: theme.surface }]}
                  accessibilityLabel="Command"
                  testID="terminal-input"
                />
                <Pressable
                  accessibilityRole="button"
                  accessibilityLabel="Send"
                  disabled={!live}
                  onPress={submit}
                  style={({ pressed }) => [styles.send, { backgroundColor: theme.accent, opacity: !live ? 0.4 : pressed ? 0.8 : 1 }]}
                  testID="terminal-send"
                >
                  <Icon icon={ArrowUp} color={theme.accentInk} size={20} />
                </Pressable>
              </View>
              <KeysRow onKey={key} disabled={!live} />
            </View>
          ) : meta ? (
            <View style={[styles.exited, { backgroundColor: theme.bg, borderTopColor: theme.line, paddingBottom: insets.bottom }]}>
              <T variant="monoSm" color={theme.mut} testID="terminal-exited">
                Exited · output kept on {hostName}
              </T>
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
  dock: {
    minHeight: 56,
    flexDirection: "row",
    alignItems: "center",
    gap: space.md,
    paddingHorizontal: space.lg,
    borderTopWidth: StyleSheet.hairlineWidth,
  },
  dockTitle: { fontFamily: fonts.sansMedium },
  dots: { flexDirection: "row", gap: space.xs },
  dockDot: { width: 7, height: 7, borderRadius: radius.full },
  sheetHead: { gap: 3 },
  sheetList: { marginHorizontal: -space.lg, flexGrow: 0, borderTopWidth: StyleSheet.hairlineWidth },
  sheetEmpty: { paddingHorizontal: space.lg, paddingTop: space.xl, paddingBottom: space.sm, gap: space.sm },
  row: {
    minHeight: 76,
    flexDirection: "row",
    alignItems: "flex-start",
    gap: space.md,
    paddingHorizontal: space.lg,
    paddingVertical: space.md + 2,
    borderBottomWidth: StyleSheet.hairlineWidth,
  },
  rowDot: { width: 8, height: 8, borderRadius: radius.full, marginTop: 6 },
  rowText: { flex: 1, gap: space.xs },
  rowHead: { flexDirection: "row", alignItems: "baseline", gap: space.md },
  rowTitle: { flexShrink: 1, fontFamily: fonts.monoMedium, fontSize: 14, lineHeight: 20 },
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
  inputRow: { flexDirection: "row", alignItems: "center", gap: space.sm },
  input: { flex: 1, height: 44, borderRadius: radius.md, paddingHorizontal: space.md },
  send: { width: 44, height: 44, borderRadius: radius.full, alignItems: "center", justifyContent: "center" },
  exited: { minHeight: 56, justifyContent: "center", paddingHorizontal: space.lg, borderTopWidth: StyleSheet.hairlineWidth },
});
