// The Session screen's pieces (DESIGN.md, Screen 5 and Components): the
// transcript's rows, the Now bar and the composer. They draw what they are
// given; the screen decides what that is.

import { useEffect, useState, type ReactNode } from "react";
import { ActivityIndicator, Pressable, ScrollView, StyleSheet, TextInput, View, useWindowDimensions } from "react-native";
import type { ToolItem } from "../sessions/transcript";
import { Button, Sheet, StatusDot, T, useReduceMotion } from "./components";
import { ArrowUp, Icon } from "./icons";
import { fonts, MONO_MAX_SCALE, radius, space, type, useTheme } from "./tokens";

/** The streaming caret: blinks once a second (static under Reduce Motion). */
export function Caret() {
  const theme = useTheme();
  const reduce = useReduceMotion();
  const [on, setOn] = useState(true);
  useEffect(() => {
    if (reduce) {
      setOn(true);
      return;
    }
    const t = setInterval(() => setOn((v) => !v), 500);
    return () => clearInterval(t);
  }, [reduce]);
  return (
    <T color={on ? theme.accent : "transparent"} testID="caret">
      {" ▍"}
    </T>
  );
}

/** What the user sent: right-aligned, on `surface`. */
export function UserMessage({ text }: { text: string }) {
  const theme = useTheme();
  return (
    <View style={[styles.user, { backgroundColor: theme.surface }]} testID="user-message">
      <T selectable>{text}</T>
    </View>
  );
}

/** The agent's reasoning: one collapsed mono row; tap to read it. */
export function ThoughtRow({ text, streaming }: { text: string; streaming: boolean }) {
  const theme = useTheme();
  const [open, setOpen] = useState(false);
  return (
    <View style={styles.rowGap}>
      <Pressable
        accessibilityRole="button"
        accessibilityState={{ expanded: open }}
        onPress={() => setOpen((o) => !o)}
        disabled={!text}
        style={styles.monoRow}
        hitSlop={8}
        testID="thought"
      >
        <T variant="mono" color={theme.mut}>
          {streaming ? "◦ Thinking…" : open ? "▾ Thought" : "◦ Thought"}
        </T>
      </Pressable>
      {open && text ? (
        <View style={[styles.thought, { borderLeftColor: theme.line }]}>
          <T color={theme.mut} selectable>
            {text}
          </T>
        </View>
      ) : null}
    </View>
  );
}

function ToolGlyph({ state }: { state: ToolItem["state"] }) {
  const theme = useTheme();
  if (state === "running") {
    return (
      <View style={styles.glyph}>
        <StatusDot state="work" />
      </View>
    );
  }
  const [g, color] = {
    done: ["✓", theme.done],
    failed: ["✕", theme.err],
    idle: ["·", theme.faint],
  }[state];
  return (
    <T variant="mono" color={color} style={styles.glyph}>
      {g}
    </T>
  );
}

/** One tool call in one mono row: status, kind, target, diff counts. */
export function ToolRow({ tool, onPress }: { tool: ToolItem; onPress?(): void }) {
  const theme = useTheme();
  const detail = !!(tool.input || tool.output);
  return (
    <Pressable
      accessibilityRole={detail ? "button" : undefined}
      accessibilityLabel={`${tool.verb ?? ""} ${tool.target}, ${tool.state}`}
      onPress={detail ? onPress : undefined}
      disabled={!detail}
      style={({ pressed }) => [styles.monoRow, pressed && { backgroundColor: theme.surface }]}
      testID="tool-row"
    >
      <ToolGlyph state={tool.state} />
      <T variant="mono" numberOfLines={1} style={styles.shrink}>
        {tool.verb ? <T variant="mono">{tool.verb} </T> : null}
        <T variant="mono" color={tool.verb ? theme.mut : theme.fg}>
          {tool.target}
        </T>
      </T>
      {tool.added || tool.removed ? (
        <T variant="monoSm" style={styles.tabular}>
          <T variant="monoSm" color={theme.done}>{`+${tool.added} `}</T>
          <T variant="monoSm" color={theme.err}>{`−${tool.removed}`}</T>
        </T>
      ) : null}
    </Pressable>
  );
}

/** Three or more tool calls in a row: `▸ n tool calls`, open to list them. */
export function ToolGroup({ tools, onOpen }: { tools: ToolItem[]; onOpen(tool: ToolItem): void }) {
  const theme = useTheme();
  const [open, setOpen] = useState(false);
  const running = tools.some((t) => t.state === "running");
  const failed = tools.filter((t) => t.state === "failed").length;
  return (
    <View>
      <Pressable
        accessibilityRole="button"
        accessibilityState={{ expanded: open }}
        onPress={() => setOpen((o) => !o)}
        style={({ pressed }) => [styles.monoRow, pressed && { backgroundColor: theme.surface }]}
        testID="tool-group"
      >
        <T variant="mono" color={theme.mut} style={styles.glyph}>
          {open ? "▾" : "▸"}
        </T>
        <T variant="mono" numberOfLines={1} style={styles.shrink}>
          {`${tools.length} tool calls`}
          {failed ? <T variant="mono" color={theme.err}>{` · ${failed} failed`}</T> : null}
        </T>
        {running ? <StatusDot state="work" /> : null}
      </Pressable>
      {open ? tools.map((t, i) => <ToolRow key={i} tool={t} onPress={() => onOpen(t)} />) : null}
    </View>
  );
}

const isDiff = (text: string) => text.startsWith("--- ") && text.includes("\n+++ ");

function Output({ text, error }: { text: string; error?: boolean }) {
  const theme = useTheme();
  const diff = !error && isDiff(text);
  return (
    <ScrollView horizontal style={[styles.output, { backgroundColor: theme.code }]} contentContainerStyle={styles.outputContent}>
      <T variant="mono" color={error ? theme.err : theme.fg} selectable>
        {diff
          ? text.split("\n").map((line, i) => (
              <T
                key={i}
                variant="mono"
                color={
                  line.startsWith("+") && !line.startsWith("+++")
                    ? theme.done
                    : line.startsWith("-") && !line.startsWith("---")
                      ? theme.err
                      : theme.fg
                }
              >
                {line + "\n"}
              </T>
            ))
          : text}
      </T>
    </ScrollView>
  );
}

/** A tool call's input and output (a command's output, a diff). */
export function ToolSheet({ tool, onClose }: { tool: ToolItem | null; onClose(): void }) {
  const theme = useTheme();
  const { height } = useWindowDimensions();
  return (
    <Sheet visible={tool !== null} onClose={onClose}>
      {tool ? (
        <>
          <T variant="mono" numberOfLines={3}>
            {tool.verb ? `${tool.verb} ` : ""}
            <T variant="mono" color={theme.mut}>
              {tool.target}
            </T>
          </T>
          <ScrollView style={{ maxHeight: height * 0.6 }} contentContainerStyle={styles.sheetBody}>
            {tool.input ? <Output text={tool.input} /> : null}
            {tool.output ? <Output text={tool.output} error={tool.state === "failed"} /> : null}
          </ScrollView>
          <Button kind="secondary" tall label="Close" onPress={onClose} />
        </>
      ) : null}
    </Sheet>
  );
}

/** An error the agent reported: `err` left edge, the message. */
export function ErrorRow({ message }: { message: string }) {
  const theme = useTheme();
  return (
    <View style={[styles.error, { backgroundColor: theme.surface, borderLeftColor: theme.err }]} testID="session-error">
      <T variant="callout" color={theme.err}>
        Error
      </T>
      <T variant="mono" selectable>
        {message}
      </T>
    </View>
  );
}

/** How the last turn ended, in one mono row. */
export function FooterRow({ text, stopped }: { text: string; stopped: boolean }) {
  const theme = useTheme();
  return (
    <View style={styles.monoRow} testID="turn-footer">
      <T variant="mono" color={stopped ? theme.mut : theme.done} style={styles.glyph}>
        {stopped ? "■" : "✓"}
      </T>
      <T variant="mono" color={theme.mut} style={[styles.shrink, styles.tabular]}>
        {text}
      </T>
    </View>
  );
}

/**
 * What the agent is doing now, above the composer: a dot, one mono line, a
 * timer and the one action that fits (Stop, Review).
 */
export function NowBar({
  tone,
  dot,
  text,
  time,
  action,
}: {
  tone: "plain" | "need";
  dot: "work" | "need" | "off" | null;
  text: string;
  time?: string | null;
  action?: ReactNode;
}) {
  const theme = useTheme();
  return (
    <View
      style={[styles.now, { backgroundColor: tone === "need" ? `${theme.need}26` : theme.surface }]}
      testID="now-bar"
      accessibilityLiveRegion="polite"
    >
      {dot ? <StatusDot state={dot} /> : null}
      <T variant="mono" numberOfLines={1} color={tone === "need" ? theme.need : theme.fg} style={styles.shrink} testID="now-text">
        {text}
      </T>
      {time ? (
        <T variant="mono" color={theme.mut} style={styles.tabular} testID="now-time">
          {time}
        </T>
      ) : null}
      <View style={styles.flexSpacer} />
      {action}
    </View>
  );
}

/** A small button for the Now bar; its touch target still reaches 44pt. */
export function NowAction({
  label,
  onPress,
  kind,
  disabled,
  loading,
  testID,
}: {
  label: string;
  onPress(): void;
  kind: "inverse" | "need";
  disabled?: boolean;
  loading?: boolean;
  testID?: string;
}) {
  const theme = useTheme();
  const [bg, fg] = kind === "need" ? [theme.need, theme.needInk] : [theme.fg, theme.bg];
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={label}
      accessibilityState={{ disabled: !!(disabled || loading), busy: !!loading }}
      onPress={onPress}
      disabled={disabled || loading}
      hitSlop={{ top: 8, bottom: 8, left: 8, right: 8 }}
      style={({ pressed }) => [styles.nowAction, { backgroundColor: bg, opacity: disabled ? 0.4 : pressed ? 0.8 : 1 }]}
      testID={testID}
    >
      {loading ? <ActivityIndicator size="small" color={fg} /> : (
        <T variant="callout" color={fg}>
          {label}
        </T>
      )}
    </Pressable>
  );
}

/** The message box: multiline, round accent send. */
export function Composer({
  value,
  onChange,
  onSend,
  placeholder,
  disabled,
  sending,
}: {
  value: string;
  onChange(text: string): void;
  onSend(): void;
  placeholder: string;
  disabled?: boolean;
  sending?: boolean;
}) {
  const theme = useTheme();
  const can = !disabled && !sending && value.trim().length > 0;
  return (
    <View style={[styles.composer, { backgroundColor: theme.surface, opacity: disabled ? 0.6 : 1 }]} testID="composer">
      <TextInput
        value={value}
        onChangeText={onChange}
        placeholder={placeholder}
        placeholderTextColor={theme.faint}
        multiline
        editable={!disabled && !sending}
        style={[type.body, styles.input, { color: theme.fg }]}
        maxFontSizeMultiplier={MONO_MAX_SCALE * 1.5}
        accessibilityLabel="Message"
        testID="composer-input"
      />
      <Pressable
        accessibilityRole="button"
        accessibilityLabel="Send"
        accessibilityState={{ disabled: !can, busy: !!sending }}
        onPress={onSend}
        disabled={!can}
        hitSlop={6}
        style={({ pressed }) => [
          styles.send,
          { backgroundColor: can || sending ? theme.accent : theme.raised, opacity: pressed ? 0.8 : 1 },
        ]}
        testID="composer-send"
      >
        {sending ? <ActivityIndicator size="small" color={theme.accentInk} /> : (
          <Icon icon={ArrowUp} color={can ? theme.accentInk : theme.faint} size={20} />
        )}
      </Pressable>
    </View>
  );
}

const styles = StyleSheet.create({
  shrink: { flexShrink: 1 },
  flexSpacer: { flex: 1 },
  tabular: { fontVariant: ["tabular-nums"] },
  rowGap: { gap: space.xs },
  user: {
    alignSelf: "flex-end",
    maxWidth: "85%",
    borderTopLeftRadius: 16,
    borderTopRightRadius: 16,
    borderBottomRightRadius: 4,
    borderBottomLeftRadius: 16,
    paddingHorizontal: space.md,
    paddingVertical: space.sm + 2,
  },
  monoRow: { minHeight: 32, flexDirection: "row", alignItems: "center", gap: space.sm, borderRadius: radius.sm },
  glyph: { width: 16, textAlign: "center", alignItems: "center" },
  thought: { borderLeftWidth: 2, paddingLeft: space.md, marginLeft: space.xs },
  output: { borderRadius: radius.md, flexGrow: 0 },
  outputContent: { padding: space.md },
  sheetBody: { gap: space.md },
  error: { borderRadius: radius.lg, borderLeftWidth: 3, padding: space.md, gap: space.xs },
  now: {
    height: 44,
    flexDirection: "row",
    alignItems: "center",
    gap: space.sm,
    paddingLeft: space.lg,
    paddingRight: space.sm,
  },
  nowAction: {
    height: 30,
    minWidth: 56,
    borderRadius: radius.md,
    paddingHorizontal: space.md,
    alignItems: "center",
    justifyContent: "center",
  },
  composer: {
    marginHorizontal: space.md,
    marginTop: space.sm,
    borderRadius: radius.xl,
    flexDirection: "row",
    alignItems: "flex-end",
    paddingLeft: space.md,
    paddingRight: space.xs + 2,
    paddingVertical: space.xs + 2,
    gap: space.sm,
  },
  input: { flex: 1, minHeight: 40, maxHeight: 140, paddingTop: 9, paddingBottom: 9, fontFamily: fonts.sans },
  send: { width: 36, height: 36, borderRadius: radius.full, alignItems: "center", justifyContent: "center" },
});
