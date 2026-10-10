// The Session screen's pieces (DESIGN.md, Screen 5 and Components): the
// transcript's rows, the Now bar and the composer. They draw what they are
// given; the screen decides what that is.

import { useEffect, useState, type ReactNode } from "react";
import { ActivityIndicator, Pressable, ScrollView, StyleSheet, TextInput, View, useWindowDimensions } from "react-native";
import { basename, formatSize, type AgentImage, type AttachmentBlock, type ChangedFile } from "@zwiim/sdk";
import type { PendingFile } from "../attachments/pending";
import type { ToolItem } from "../sessions/transcript";
import { Button, Sheet, StatusDot, T, useReduceMotion } from "./components";
import { ArrowUp, CircleAlert, FileText, Icon, Image, Paperclip, X } from "./icons";
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
export function UserMessage({
  text,
  files = [],
  onOpenFile,
}: {
  text: string;
  /** The files it carried, above the text. */
  files?: readonly AttachmentBlock[];
  onOpenFile?(file: AttachmentBlock): void;
}) {
  const theme = useTheme();
  return (
    <View style={styles.rowGap}>
      {files.length > 0 ? (
        <View style={styles.sentFiles} testID="sent-attachments">
          {files.map((f) => (
            <SentFileChip key={f.attachment_id} file={f} onPress={onOpenFile ? () => onOpenFile(f) : undefined} />
          ))}
        </View>
      ) : null}
      {text ? (
        <View style={[styles.user, { backgroundColor: theme.surface }]} testID="user-message">
          <T selectable>{text}</T>
        </View>
      ) : null}
    </View>
  );
}

/** A file sent with a message: name and size; tap to open it in another app. "removed" once it is gone. */
function SentFileChip({ file, onPress }: { file: AttachmentBlock; onPress?(): void }) {
  const theme = useTheme();
  const gone = file.size === undefined;
  const open = onPress && !gone ? onPress : undefined;
  return (
    <Pressable
      accessibilityRole={open ? "button" : undefined}
      accessibilityLabel={gone ? `${file.name}, removed` : `Open ${file.name}`}
      disabled={!open}
      onPress={open}
      hitSlop={6}
      style={({ pressed }) => [styles.fileChip, { backgroundColor: pressed ? theme.raised : theme.surface }]}
      testID="sent-attachment"
    >
      <Icon icon={file.mime_type.startsWith("image/") ? Image : FileText} color={theme.mut} size={13} />
      <T variant="monoSm" color={gone ? theme.mut : theme.fg} numberOfLines={1} style={styles.shrink}>
        {file.name}
      </T>
      <T variant="monoSm" color={theme.faint}>
        {gone ? "removed" : formatSize(file.size!)}
      </T>
    </Pressable>
  );
}

/** An image the agent produced; tap to preview it. */
export function AgentImageChip({ image, onPress }: { image: AgentImage; onPress?(): void }) {
  const theme = useTheme();
  return (
    <Pressable
      accessibilityRole={onPress ? "button" : undefined}
      accessibilityLabel={`Open ${image.name}`}
      disabled={!onPress}
      onPress={onPress}
      hitSlop={6}
      style={({ pressed }) => [styles.fileChip, { alignSelf: "flex-start", backgroundColor: pressed ? theme.raised : theme.surface }]}
      testID="agent-image"
    >
      <Icon icon={Image} color={theme.mut} size={13} />
      <T variant="monoSm" color={theme.fg} numberOfLines={1} style={styles.shrink}>
        {image.name}
      </T>
    </Pressable>
  );
}

/** A file chosen for the next message: uploading, failed, or waiting to send. */
function PendingFileChip({ file, disabled, onRemove }: { file: PendingFile; disabled?: boolean; onRemove(): void }) {
  const theme = useTheme();
  const failed = file.status === "failed";
  return (
    <View
      style={[styles.fileChip, styles.pendingChip, { backgroundColor: theme.raised, borderColor: failed ? theme.err : "transparent" }]}
      testID="pending-attachment"
    >
      {file.status === "uploading" ? (
        <ActivityIndicator size="small" color={theme.mut} accessibilityLabel="Uploading" />
      ) : (
        <Icon icon={failed ? CircleAlert : file.mime_type.startsWith("image/") ? Image : FileText} color={failed ? theme.err : theme.mut} size={13} />
      )}
      <T variant="monoSm" numberOfLines={1} style={styles.shrink}>
        {file.name}
      </T>
      <T variant="monoSm" color={failed ? theme.err : theme.faint} numberOfLines={1}>
        {failed ? "Upload failed" : formatSize(file.size)}
      </T>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={`Remove ${file.name}`}
        disabled={disabled || file.status === "uploading"}
        onPress={onRemove}
        hitSlop={8}
        testID="pending-remove"
      >
        <Icon icon={X} color={theme.mut} size={13} />
      </Pressable>
    </View>
  );
}

/** The files chosen for a message, with why any were refused. */
export function PendingFiles({
  files,
  errors,
  disabled,
  onRemove,
}: {
  files: readonly PendingFile[];
  errors: readonly string[];
  disabled?: boolean;
  onRemove(id: string): void;
}) {
  const theme = useTheme();
  if (files.length === 0 && errors.length === 0) return null;
  return (
    <View style={styles.rowGap}>
      {files.length > 0 ? (
        <View style={styles.pendingFiles} testID="pending-attachments">
          {files.map((f) => (
            <PendingFileChip key={f.id} file={f} disabled={disabled} onRemove={() => onRemove(f.id)} />
          ))}
        </View>
      ) : null}
      {errors.map((e) => (
        <T key={e} variant="monoSm" color={theme.err} accessibilityRole="alert" testID="attachment-error">
          {e}
        </T>
      ))}
    </View>
  );
}

/** Where to attach from: the photo library or Files. */
export function AttachSheet({
  visible,
  onClose,
  onPick,
}: {
  visible: boolean;
  onClose(): void;
  onPick(from: "photos" | "files"): void;
}) {
  return (
    <Sheet visible={visible} onClose={onClose}>
      <View style={styles.sheetBody}>
        <T variant="title">Attach</T>
        <Button tall kind="secondary" icon={Image} label="Photos" onPress={() => onPick("photos")} testID="attach-photos" />
        <Button tall kind="secondary" icon={FileText} label="Files" onPress={() => onPick("files")} testID="attach-files" />
      </View>
    </Sheet>
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
export function ToolRow({ tool, onPress, opens }: { tool: ToolItem; onPress?(): void; opens?: boolean }) {
  const theme = useTheme();
  const pressable = opens || !!(tool.input || tool.output);
  return (
    <Pressable
      accessibilityRole={pressable ? "button" : undefined}
      accessibilityLabel={`${tool.verb ?? ""} ${tool.target}, ${tool.state}`}
      accessibilityHint={opens ? "Opens the file" : undefined}
      onPress={pressable ? onPress : undefined}
      disabled={!pressable}
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
export function ToolGroup({
  tools,
  onOpen,
  opens,
}: {
  tools: ToolItem[];
  onOpen(tool: ToolItem): void;
  opens?(tool: ToolItem): boolean;
}) {
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
      {open ? tools.map((t, i) => <ToolRow key={i} tool={t} onPress={() => onOpen(t)} opens={opens?.(t)} />) : null}
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

/** Files a turn's row shows before `+N more`, and how many each tap adds. */
const FILES_SHOWN = 2;

/**
 * Under each answered prompt: how long the turn took and when it ended (or
 * that it was stopped), then the files it changed as chips, two at first and
 * two more per tap on `+N more`. `fileOf` says which ones open.
 */
export function FooterRow({
  text,
  stopped,
  files,
  fileOf,
  onOpenFile,
}: {
  text: string;
  stopped: boolean;
  files: ChangedFile[];
  fileOf(path: string): string | null;
  onOpenFile(path: string): void;
}) {
  const theme = useTheme();
  const [shown, setShown] = useState(FILES_SHOWN);
  const rest = files.slice(shown);
  const sum = (key: "added" | "removed") => rest.reduce((n, f) => n + f[key], 0);
  return (
    <View testID="turn-footer">
      <View style={styles.monoRow}>
        <T variant="mono" color={stopped ? theme.mut : theme.done} style={styles.glyph}>
          {stopped ? "■" : "✓"}
        </T>
        <T variant="mono" color={theme.mut} style={[styles.shrink, styles.tabular]} testID="turn-time">
          {text}
        </T>
      </View>
      {files.length ? (
        <View style={styles.fileChips}>
          {files.slice(0, shown).map((f) => {
            const path = fileOf(f.path);
            return (
              <FileChip
                key={f.path}
                label={basename(f.path)}
                added={f.added}
                removed={f.removed}
                onPress={path ? () => onOpenFile(path) : undefined}
                a11y={f.path}
              />
            );
          })}
          {rest.length ? (
            <FileChip
              label={`+${rest.length} more`}
              added={sum("added")}
              removed={sum("removed")}
              onPress={() => setShown((n) => n + FILES_SHOWN)}
              a11y={`${rest.length} more files`}
              more
            />
          ) : files.length > FILES_SHOWN ? (
            <FileChip label="Show less" onPress={() => setShown(FILES_SHOWN)} a11y="Show fewer files" more />
          ) : null}
        </View>
      ) : null}
    </View>
  );
}

/** A changed file, or `+N more`, as a 28pt chip: icon, mono label, `+n −n`. */
function FileChip({
  label,
  added,
  removed,
  onPress,
  a11y,
  more,
}: {
  label: string;
  added?: number;
  removed?: number;
  onPress?(): void;
  a11y: string;
  more?: boolean;
}) {
  const theme = useTheme();
  return (
    <Pressable
      accessibilityRole={onPress ? "button" : undefined}
      accessibilityLabel={a11y}
      disabled={!onPress}
      onPress={onPress}
      hitSlop={6}
      style={({ pressed }) => [styles.fileChip, { backgroundColor: pressed ? theme.raised : theme.surface }]}
      testID={more ? "turn-more" : "turn-file"}
    >
      {more ? null : <Icon icon={FileText} color={theme.mut} size={13} />}
      <T variant="monoSm" color={more ? theme.mut : theme.fg} numberOfLines={1} style={styles.shrink}>
        {label}
      </T>
      {added || removed ? (
        <T variant="monoSm" style={styles.tabular}>
          <T variant="monoSm" color={theme.done}>{`+${added ?? 0} `}</T>
          <T variant="monoSm" color={theme.err}>{`−${removed ?? 0}`}</T>
        </T>
      ) : null}
    </Pressable>
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

/**
 * The message box: multiline input over a row of the session's controls and
 * the round accent send. An amber border marks an unattended session.
 */
export function Composer({
  value,
  onChange,
  onSend,
  placeholder,
  disabled,
  sending,
  controls,
  note,
  border,
  files = [],
  fileErrors = [],
  onAttach,
  onRemoveFile,
}: {
  value: string;
  onChange(text: string): void;
  onSend(): void;
  placeholder: string;
  disabled?: boolean;
  sending?: boolean;
  /** Left of send: the session's chips. */
  controls?: ReactNode;
  /** One mono line under the input (why the controls are locked). */
  note?: string | null;
  border?: string;
  /** Files chosen for this message, over the input. */
  files?: readonly PendingFile[];
  /** Why picked files were refused: too large, too many. */
  fileErrors?: readonly string[];
  /** Shows the paperclip; absent when the Host takes no attachments. */
  onAttach?(): void;
  onRemoveFile?(id: string): void;
}) {
  const theme = useTheme();
  const can = !disabled && !sending && (value.trim().length > 0 || files.length > 0);
  return (
    <View
      style={[
        styles.composer,
        { backgroundColor: theme.surface, borderColor: border ?? "transparent", opacity: disabled ? 0.6 : 1 },
      ]}
      testID="composer"
    >
      <PendingFiles files={files} errors={fileErrors} disabled={sending} onRemove={(id) => onRemoveFile?.(id)} />
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
      {note ? (
        <T variant="monoSm" color={theme.faint} numberOfLines={1} style={styles.composerNote} testID="composer-note">
          {note}
        </T>
      ) : null}
      <View style={styles.composerRow}>
        {onAttach ? (
          <Pressable
            accessibilityRole="button"
            accessibilityLabel="Attach files"
            onPress={onAttach}
            disabled={disabled || sending}
            hitSlop={6}
            style={({ pressed }) => [styles.attach, { opacity: disabled || sending ? 0.4 : pressed ? 0.6 : 1 }]}
            testID="composer-attach"
          >
            <Icon icon={Paperclip} color={theme.mut} size={18} />
          </Pressable>
        ) : null}
        <View style={styles.composerControls}>{controls}</View>
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
  fileChips: { flexDirection: "row", flexWrap: "wrap", gap: space.xs, paddingLeft: 16 + space.sm, paddingBottom: space.xs },
  fileChip: {
    height: 28,
    maxWidth: "100%",
    flexDirection: "row",
    alignItems: "center",
    gap: space.xs,
    paddingHorizontal: space.sm,
    borderRadius: radius.sm,
  },
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
    borderWidth: 1,
    paddingLeft: space.md,
    paddingRight: space.xs + 2,
    paddingTop: space.xs,
    paddingBottom: space.xs + 2,
    gap: space.xs,
  },
  input: { minHeight: 40, maxHeight: 140, paddingTop: 9, paddingBottom: 4, paddingRight: space.sm, fontFamily: fonts.sans },
  composerNote: { paddingBottom: space.xxs },
  composerRow: { flexDirection: "row", alignItems: "center", gap: space.sm },
  composerControls: { flex: 1, minWidth: 0, flexDirection: "row", alignItems: "center", gap: space.xs },
  send: { width: 36, height: 36, borderRadius: radius.full, alignItems: "center", justifyContent: "center" },
  attach: { width: 28, height: 36, alignItems: "center", justifyContent: "center" },
  sentFiles: { alignSelf: "flex-end", maxWidth: "85%", flexDirection: "row", flexWrap: "wrap", justifyContent: "flex-end", gap: space.xs },
  pendingFiles: { flexDirection: "row", flexWrap: "wrap", gap: space.xs, paddingTop: space.xs, paddingRight: space.sm },
  pendingChip: { borderWidth: 1 },
});
