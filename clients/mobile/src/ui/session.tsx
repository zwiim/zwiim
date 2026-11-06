// The Session screen's pieces (DESIGN.md, Screen 5 and Components): the
// transcript's rows, the Now bar and the composer. They draw what they are
// given; the screen decides what that is.

import { Fragment, useEffect, useRef, useState, type ReactNode } from "react";
import { ActivityIndicator, Animated, PanResponder, Pressable, ScrollView, StyleSheet, TextInput, View, useWindowDimensions } from "react-native";
import {
  basename,
  FORK_LABELS,
  formatSize,
  revertWarning,
  type AgentImage,
  type AgentType,
  type AttachmentBlock,
  type ChangedFile,
  type ForkTarget,
  type ForkWorktree,
  type ProgressStep,
  type RevertChoice,
} from "@zwiim/sdk";
import type { PendingFile } from "../attachments/pending";
import {
  CONTINUE_NOTE,
  continuedInLabel,
  continuingLabel,
  doneCount,
  fromLabel,
  pipWindow,
  stoppedTitle,
  type PipState,
} from "../sessions/plan";
import { forkedFromLabel, forkWorktreeLines, type TurnFork } from "../sessions/fork";
import { revertCopy } from "../sessions/revert";
import {
  RETRY_LABEL,
  RETRY_NEW_CHAT_LABEL,
  RETRY_TARGET,
  retryBusy,
  retryColors,
  retryTitle,
  runRetry,
  type RetryChoice,
} from "../sessions/unreachable";
import { useNow } from "../app/hooks";
import {
  runSummary,
  subagentLabel,
  subagentShown,
  subagentTime,
  type FoldedRow,
  type SubagentItem,
  type ToolItem,
  type TranscriptRow,
} from "../sessions/transcript";
import { Button, Sheet, StatusDot, T, useBreathing, useReduceMotion } from "./components";
import { ArrowUp, Bot, CircleAlert, FileText, GitFork, Icon, Image, Paperclip, RotateCcw, X } from "./icons";
import { fonts, PRESSED_OPACITY, radius, space, type, useTheme } from "./tokens";

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
  onRevert,
  revertDisabled,
}: {
  text: string;
  /** The files it carried, above the text. */
  files?: readonly AttachmentBlock[];
  onOpenFile?(file: AttachmentBlock): void;
  /** Offered on a message the Host can put the session back to. */
  onRevert?(): void;
  revertDisabled?: boolean;
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
      {onRevert ? <RevertAction onPress={onRevert} disabled={revertDisabled} /> : null}
    </View>
  );
}

/** "Reset to here", under a message the Host can put the session back to. */
export function RevertAction({ onPress, disabled }: { onPress(): void; disabled?: boolean }) {
  const theme = useTheme();
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel="Reset to here"
      accessibilityState={{ disabled: !!disabled }}
      disabled={disabled}
      onPress={onPress}
      hitSlop={REVERT_SLOP}
      style={({ pressed }) => [styles.revert, { opacity: disabled ? 0.4 : pressed ? PRESSED_OPACITY : 1 }]}
      testID="user-reset"
    >
      <Icon icon={RotateCcw} color={theme.mut} size={13} />
      <T variant="callout" color={theme.mut}>
        Reset to here
      </T>
    </Pressable>
  );
}

/**
 * Asks before a session goes back to before one of its messages. With `chat`
 * the chat is reset, and the files with it unless unchecked; otherwise only
 * the files are. A refusal keeps it open and reads under its title.
 */
export function RevertSheet({
  visible,
  chat,
  disabled,
  error,
  resetting,
  onReset,
  onClose,
}: {
  visible: boolean;
  chat: boolean;
  /** Offline or busy: the reset waits. */
  disabled?: boolean;
  error: string | null;
  resetting: boolean;
  onReset(choice: RevertChoice): void;
  onClose(): void;
}) {
  const theme = useTheme();
  const copy = revertCopy(chat);
  const [files, setFiles] = useState(true);
  const warning = revertWarning(chat ? { files, chat: true } : { files: true, chat: false });
  useEffect(() => {
    if (visible) setFiles(true);
  }, [visible]);
  return (
    <Sheet visible={visible} onClose={onClose}>
      <T variant="title">{copy.title}</T>
      <T color={theme.mut} testID="reset-body">
        {copy.body}
      </T>
      {copy.filesOption ? (
        <Pressable
          accessibilityRole="checkbox"
          accessibilityState={{ checked: files, disabled: resetting }}
          disabled={resetting}
          onPress={() => setFiles((f) => !f)}
          style={styles.checkRow}
          testID="reset-files"
        >
          <T variant="mono" color={files ? theme.fg : theme.mut}>
            {files ? "[x]" : "[ ]"}
          </T>
          <T>{copy.filesOption}</T>
        </Pressable>
      ) : null}
      {warning ? (
        <T color={theme.mut} testID="reset-warning">
          {warning}
        </T>
      ) : null}
      {error ? (
        <T color={theme.err} testID="reset-error">
          {error}
        </T>
      ) : null}
      <Button
        tall
        kind="destructive"
        label={copy.confirm}
        loading={resetting}
        disabled={disabled}
        onPress={() => onReset(chat ? { files, chat: true } : { files: true, chat: false })}
        testID="reset-confirm"
      />
      <Button tall kind="secondary" label="Cancel" onPress={onClose} />
    </Sheet>
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
      hitSlop={8}
      style={({ pressed }) => [styles.fileChip, { backgroundColor: pressed ? theme.raised : theme.surface }]}
      testID="sent-attachment"
    >
      <Icon icon={file.mime_type.startsWith("image/") ? Image : FileText} color={theme.mut} size={13} />
      <T variant="monoSm" color={gone ? theme.mut : theme.fg} numberOfLines={1} style={styles.shrink}>
        {file.name}
      </T>
      <T variant="monoSm" color={theme.mut}>
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
      hitSlop={8}
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
      <T variant="monoSm" color={failed ? theme.err : theme.mut} numberOfLines={1}>
        {failed ? "Upload failed" : formatSize(file.size)}
      </T>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={`Remove ${file.name}`}
        disabled={disabled || file.status === "uploading"}
        onPress={onRemove}
        // A 13pt mark; the slop makes it a 44pt target.
        hitSlop={16}
        testID="pending-remove"
      >
        <Icon icon={X} color={theme.mut} size={13} />
      </Pressable>
    </View>
  );
}

/** A fork's transcript waiting for the next message: tap to read it, X takes it back. */
function TranscriptChip({
  file,
  disabled,
  onOpen,
  onRemove,
}: {
  file: AttachmentBlock;
  disabled?: boolean;
  onOpen?(): void;
  onRemove(): void;
}) {
  const theme = useTheme();
  return (
    <View style={[styles.fileChip, styles.pendingChip, { backgroundColor: theme.raised, borderColor: "transparent" }]} testID="pending-transcript">
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={`Open ${file.name}`}
        disabled={!onOpen}
        onPress={onOpen}
        hitSlop={8}
        style={({ pressed }) => [styles.transcriptOpen, { opacity: pressed ? PRESSED_OPACITY : 1 }]}
      >
        <Icon icon={FileText} color={theme.mut} size={13} />
        <T variant="caption" numberOfLines={1} style={styles.shrink}>
          {file.name}
        </T>
      </Pressable>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={`Remove ${file.name}`}
        disabled={disabled}
        onPress={onRemove}
        // A 13pt mark; the slop makes it a 44pt target.
        hitSlop={16}
        testID="transcript-remove"
      >
        <Icon icon={X} color={theme.mut} size={13} />
      </Pressable>
    </View>
  );
}

/** The files chosen for a message, with why any were refused. */
export function PendingFiles({
  transcript,
  files,
  errors,
  disabled,
  onRemove,
}: {
  /** A fork's transcript, ahead of the files. */
  transcript?: ReactNode;
  files: readonly PendingFile[];
  errors: readonly string[];
  disabled?: boolean;
  onRemove(id: string): void;
}) {
  const theme = useTheme();
  if (!transcript && files.length === 0 && errors.length === 0) return null;
  return (
    <View style={styles.rowGap}>
      {transcript || files.length > 0 ? (
        <View style={styles.pendingFiles} testID="pending-attachments">
          {transcript}
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
  onClosed,
}: {
  visible: boolean;
  onClose(): void;
  onPick(from: "photos" | "files"): void;
  /** Once the sheet has fully gone: the moment a native picker can present. */
  onClosed?(): void;
}) {
  return (
    <Sheet visible={visible} onClose={onClose} onClosed={onClosed}>
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
      hitSlop={ROW_SLOP}
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

/**
 * Three or more tool calls and sub-agents in a row: `▸ n tool calls, m
 * sub-agents`, open to list them, each drawn by `renderRow`.
 */
export function ToolGroup({ rows, renderRow }: { rows: FoldedRow[]; renderRow(row: FoldedRow): ReactNode }) {
  const theme = useTheme();
  const [open, setOpen] = useState(false);
  const { text, failed, running } = runSummary(rows);
  return (
    <View>
      <Pressable
        accessibilityRole="button"
        accessibilityState={{ expanded: open }}
        onPress={() => setOpen((o) => !o)}
        hitSlop={ROW_SLOP}
        style={({ pressed }) => [styles.monoRow, pressed && { backgroundColor: theme.surface }]}
        testID="tool-group"
      >
        <T variant="mono" color={theme.mut} style={styles.glyph}>
          {open ? "▾" : "▸"}
        </T>
        <T variant="mono" numberOfLines={1} style={styles.shrink}>
          {text}
          {failed ? <T variant="mono" color={theme.err}>{` · ${failed} failed`}</T> : null}
        </T>
        {running ? <StatusDot state="work" /> : null}
      </Pressable>
      {open ? rows.map((r) => <Fragment key={r.key}>{renderRow(r)}</Fragment>) : null}
    </View>
  );
}

/**
 * One sub-agent: a row with its model, task, tool count and time; open to read
 * its prompt (behind its own toggle) and what it did, drawn by `renderChild`.
 * Nothing inside is drawn while it is shut.
 */
export function SubagentRow({
  agent,
  renderChild,
}: {
  agent: SubagentItem;
  renderChild(row: TranscriptRow): ReactNode;
}) {
  const theme = useTheme();
  const [open, setOpen] = useState(false);
  const [promptOpen, setPromptOpen] = useState(false);
  const shown = subagentShown(agent, open, promptOpen);
  const running = agent.state === "running";
  // Ticks here, not in the transcript's memoized rows, and only while running.
  const time = subagentTime(agent, useNow(running && agent.since !== null));
  const tools = agent.tools > 0 ? `${agent.tools} ${agent.tools === 1 ? "tool" : "tools"}` : null;
  const label = subagentLabel(agent);
  return (
    <View testID="subagent">
      <Pressable
        accessibilityRole="button"
        accessibilityState={{ expanded: open }}
        accessibilityLabel={label}
        onPress={() => setOpen((o) => !o)}
        hitSlop={ROW_SLOP}
        style={({ pressed }) => [styles.monoRow, pressed && { backgroundColor: theme.surface }]}
        testID="subagent-row"
      >
        <View style={styles.glyph}>
          <Icon icon={Bot} color={running ? theme.fg : theme.mut} size={16} />
        </View>
        <T variant="mono" numberOfLines={1} style={styles.shrink}>
          <T variant="mono">Agent</T>
          {agent.model ? <T variant="mono" color={theme.mut}>{` • ${agent.model}`}</T> : null}
          {agent.description ? <T variant="mono" color={theme.mut}>{`  ${agent.description}`}</T> : null}
        </T>
        <View style={styles.flexSpacer} />
        {tools ? (
          <T variant="monoSm" color={theme.mut} style={styles.tabular} testID="subagent-tools">
            {tools}
          </T>
        ) : null}
        {agent.tokens ? (
          <T variant="monoSm" color={theme.mut} style={styles.tabular} testID="subagent-tokens">
            {agent.tokens}
          </T>
        ) : null}
        {running ? <StatusDot state="work" /> : null}
        {time ? (
          <T
            variant="monoSm"
            color={theme.mut}
            style={styles.tabular}
            testID={running ? "subagent-elapsed" : "subagent-duration"}
          >
            {time}
          </T>
        ) : null}
        {agent.state === "failed" ? (
          <T variant="monoSm" color={theme.err} testID="subagent-failed">
            ✕ failed
          </T>
        ) : agent.state === "interrupted" ? (
          // Stopped unfinished: said plainly, muted, since nothing went wrong in it.
          <T variant="monoSm" color={theme.mut} testID="subagent-interrupted">
            interrupted
          </T>
        ) : null}
        <T variant="mono" color={theme.mut}>
          {open ? "▾" : "▸"}
        </T>
      </Pressable>
      {open ? (
        <View style={[styles.subagent, { borderLeftColor: theme.line }]} testID="subagent-body">
          {shown.promptToggle ? (
            <View style={styles.rowGap}>
              <Pressable
                accessibilityRole="button"
                accessibilityState={{ expanded: promptOpen }}
                onPress={() => setPromptOpen((o) => !o)}
                hitSlop={ROW_SLOP}
                style={styles.monoRow}
                testID="subagent-prompt-toggle"
              >
                <T variant="mono" color={theme.mut}>
                  {promptOpen ? "▾ Prompt" : "▸ Prompt"}
                </T>
              </Pressable>
              {shown.prompt ? (
                <View style={[styles.prompt, { backgroundColor: theme.code }]}>
                  <T variant="monoSm" color={theme.mut} selectable testID="subagent-prompt">
                    {shown.prompt}
                  </T>
                </View>
              ) : null}
            </View>
          ) : null}
          {shown.rows.map((row) => (
            <View key={row.key}>{renderChild(row)}</View>
          ))}
        </View>
      ) : null}
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

/**
 * The agent could not reach its provider. The Host kept the failed turn, so
 * it can be sent again here or carried into a new chat. Nothing runs until a
 * button is tapped; both hold until the op answers, and while offline.
 */
export function UnreachableCard({
  agentName,
  message,
  online,
  hostName,
  onRetry,
  onRetryInNewChat,
  onStarted,
}: {
  agentName: string;
  message: string;
  online: boolean;
  hostName: string;
  onRetry(): Promise<void>;
  /** Starts the new chat; resolves to its session id. */
  onRetryInNewChat(): Promise<string>;
  /** Opens the chat that was started. Not part of the op: a throw is not a failed retry. */
  onStarted(session_id: string): void;
}) {
  const theme = useTheme();
  const color = retryColors(theme);
  const [pending, setPending] = useState<RetryChoice | null>(null);
  const [refusal, setRefusal] = useState<string | null>(null);
  const busy = retryBusy(pending, online);
  const run = (which: RetryChoice) =>
    void (which === "retry"
      ? runRetry(which, onRetry, { pending: setPending, error: setRefusal }, { hostName })
      : runRetry(which, onRetryInNewChat, { pending: setPending, error: setRefusal }, { hostName, after: onStarted }));
  const button = (which: RetryChoice, label: string, testID: string) => (
    <Pressable
      testID={testID}
      accessibilityRole="button"
      accessibilityLabel={label}
      accessibilityState={{ disabled: busy, busy: pending === which }}
      disabled={busy}
      onPress={() => run(which)}
      style={({ pressed }) => [styles.retryButton, { opacity: busy ? 0.4 : pressed ? PRESSED_OPACITY : 1 }]}
    >
      {pending === which ? <ActivityIndicator color={color.action} size="small" /> : null}
      <T color={color.action} style={styles.retryLabel}>
        {label}
      </T>
    </Pressable>
  );
  return (
    <View style={[styles.error, { backgroundColor: color.background, borderLeftColor: color.edge }]} testID="unreachable-card">
      <T selectable>
        {retryTitle(agentName, message)}
      </T>
      <View>
        {button("retry", RETRY_LABEL, "unreachable-retry")}
        {button("new_chat", RETRY_NEW_CHAT_LABEL, "unreachable-retry-new")}
      </View>
      {refusal ? (
        <T variant="callout" color={color.refusal} testID="unreachable-refusal">
          {refusal}
        </T>
      ) : null}
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
  fork,
  forkDisabled,
  onFork,
}: {
  text: string;
  stopped: boolean;
  files: ChangedFile[];
  fileOf(path: string): string | null;
  onOpenFile(path: string): void;
  /** Absent when the Host offers no fork of this turn. */
  fork?: TurnFork | null;
  /** Why the actions are off; null when they're on. */
  forkDisabled?: string | null;
  onFork?(fork: TurnFork): void;
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
        {fork && onFork ? (
          <Pressable
            accessibilityRole="button"
            accessibilityLabel="Fork"
            accessibilityHint={forkDisabled ?? undefined}
            accessibilityState={{ disabled: !!forkDisabled }}
            disabled={!!forkDisabled}
            onPress={() => onFork(fork)}
            style={({ pressed }) => [styles.forkIcon, { opacity: forkDisabled ? 0.4 : pressed ? PRESSED_OPACITY : 1 }]}
            testID="fork-open"
          >
            <Icon icon={GitFork} color={theme.mut} size={18} />
          </Pressable>
        ) : null}
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

/** The fork options a turn offers, then the word a fork needs first, or why one was refused. */
export function ForkSheet({
  visible,
  choices,
  onPick,
  target,
  note,
  error,
  forking,
  disabled,
  onFork,
  onClose,
}: {
  visible: boolean;
  /** Set while choosing: what the turn offers. */
  choices: readonly ForkTarget[] | null;
  onPick(target: ForkTarget): void;
  target: ForkTarget;
  note: string | null;
  error: string | null;
  forking: boolean;
  /** Offline or busy: the fork waits. */
  disabled?: boolean;
  onFork(): void;
  onClose(): void;
}) {
  const theme = useTheme();
  return (
    <Sheet visible={visible} onClose={onClose}>
      <T variant="title">{choices ? "Fork" : FORK_LABELS[target]}</T>
      {note ? (
        <T color={theme.mut} testID="fork-note">
          {note}
        </T>
      ) : null}
      {error ? (
        <T color={theme.err} testID="fork-error">
          {error}
        </T>
      ) : null}
      {choices ? (
        choices.map((choice) => (
          <Button key={choice} tall label={FORK_LABELS[choice]} disabled={disabled} onPress={() => onPick(choice)} testID={`fork-${choice}`} />
        ))
      ) : (
        <Button tall label={FORK_LABELS[target]} loading={forking} disabled={disabled} onPress={onFork} testID="fork-confirm" />
      )}
      <Button tall kind="secondary" label="Cancel" onPress={onClose} />
    </Sheet>
  );
}

/** A fork nobody has written in yet: the worktree it made, or where the new chat is. */
export function ForkStart({ worktree, workspaceName }: { worktree?: ForkWorktree | null; workspaceName?: string }) {
  const theme = useTheme();
  if (!worktree) {
    return (
      <T variant="callout" color={theme.mut} style={styles.forkStart} testID="fork-start">
        {workspaceName ? `New chat in ${workspaceName}` : "New chat"}
      </T>
    );
  }
  const { title, detail } = forkWorktreeLines(worktree);
  return (
    <View style={[styles.forkCard, { backgroundColor: theme.surface }]} testID="fork-worktree">
      <T variant="body">{title}</T>
      <T variant="monoSm" color={theme.mut}>
        {detail}
      </T>
    </View>
  );
}

/** Over a forked session's first message: where it came from. */
export function ForkedFromRow({ title, onOpen }: { title: string | null; onOpen?(): void }) {
  const theme = useTheme();
  const label = forkedFromLabel(title);
  if (!onOpen) {
    return (
      <View style={styles.linkRow} testID="forked-from">
        <T variant="callout" color={theme.mut}>
          {label}
        </T>
      </View>
    );
  }
  return (
    <Pressable
      accessibilityRole="link"
      accessibilityLabel={label}
      onPress={onOpen}
      style={({ pressed }) => [styles.linkRow, { opacity: pressed ? PRESSED_OPACITY : 1 }]}
      testID="forked-from"
    >
      <T variant="callout" color={theme.accent}>
        {label}
      </T>
    </Pressable>
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
      hitSlop={8}
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
      style={({ pressed }) => [styles.nowAction, { backgroundColor: bg, opacity: disabled ? 0.4 : pressed ? PRESSED_OPACITY : 1 }]}
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

/** The color of a step's pip: done green, the one in hand muted, the stop an accent ring, the rest faint. */
function pipColor(state: PipState, theme: ReturnType<typeof useTheme>): string {
  return state === "done" ? theme.done : state === "current" ? theme.mut : state === "stopped" ? theme.accent : theme.faint;
}

/** The pip of the step in hand: a `mut` dot that breathes like the working mark, without its halo. */
function CurrentPip() {
  const theme = useTheme();
  const opacity = useBreathing(true);
  return <Animated.View style={[styles.pip, { backgroundColor: theme.mut, opacity }]} testID="pip-current" />;
}

/** One pip per step. Stopped is a hollow accent ring (not an error); pending a hollow faint one. */
function Pips({ states }: { states: readonly PipState[] }) {
  const theme = useTheme();
  return (
    <View style={styles.pips} accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
      {pipWindow(states).map((state, i) =>
        state === "current" ? (
          <CurrentPip key={i} />
        ) : (
          <View
            key={i}
            style={[
              styles.pip,
              state === "stopped" || state === "pending"
                ? { borderWidth: 1.5, borderColor: pipColor(state, theme) }
                : { backgroundColor: pipColor(state, theme) },
            ]}
            testID={`pip-${state}`}
          />
        ),
      )}
    </View>
  );
}

/**
 * The plan's steps, one row each: a mark, then the title. The step the agent
 * is on, or stopped at, reads in full color; the one in hand shows `activity`
 * (what the agent is doing) under its title.
 */
export function StepList({
  steps,
  states,
  activity,
}: {
  steps: readonly ProgressStep[];
  states: readonly PipState[];
  activity?: string | null;
}) {
  const theme = useTheme();
  return (
    <View style={styles.steps} testID="step-list">
      {steps.map((step, i) => {
        const state = states[i] ?? "pending";
        const here = state === "current";
        return (
          <View key={i} style={styles.step} testID={here ? "step-row-current" : "step-row"}>
            <T variant="mono" color={pipColor(state, theme)} style={styles.glyph}>
              {state === "done" ? "✓" : state === "stopped" ? "◯" : here ? "●" : "○"}
            </T>
            <View style={styles.shrink}>
              <T color={state === "pending" ? theme.mut : theme.fg} style={here ? styles.stepHere : undefined}>
                {step.title}
              </T>
              {here && activity ? (
                <T variant="mono" color={theme.mut} numberOfLines={1} testID="step-activity">
                  {activity}
                </T>
              ) : null}
            </View>
          </View>
        );
      })}
    </View>
  );
}

/**
 * The plan's status while the agent works through it, in the Now bar's place,
 * two lines. First: the pips, `N of M`, the time, and Stop. Second: the step in
 * hand, then what the agent is doing, in mono. A tap or a swipe up opens the steps.
 */
export function StepMeter({
  title,
  activity,
  count,
  time,
  states,
  action,
  onOpen,
}: {
  title: string;
  activity?: string | null;
  count: string;
  time?: string | null;
  states: readonly PipState[];
  action?: ReactNode;
  onOpen(): void;
}) {
  const theme = useTheme();
  const open = useRef(onOpen);
  useEffect(() => {
    open.current = onOpen;
  });
  const swipe = useRef(
    PanResponder.create({
      // Capture: the body's Pressable would otherwise keep a vertical drag as its own.
      onMoveShouldSetPanResponderCapture: (_, g) => g.dy < -10 && Math.abs(g.dy) > 2 * Math.abs(g.dx),
      onPanResponderRelease: (_, g) => {
        if (g.dy < -24) open.current();
      },
    }),
  ).current;
  return (
    <View
      style={[styles.meter, { backgroundColor: theme.surface }]}
      testID="step-meter"
      accessibilityLiveRegion="polite"
      {...swipe.panHandlers}
    >
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={`${title}. ${count}. Show tasks`}
        onPress={onOpen}
        style={styles.meterBody}
        testID="step-meter-open"
      >
        <View style={styles.meterTop}>
          <Pips states={states} />
          <T variant="mono" color={theme.mut} style={styles.tabular} testID="step-meter-count">
            {count}
          </T>
          {time ? (
            <T variant="mono" color={theme.mut} style={styles.tabular} testID="now-time">
              {time}
            </T>
          ) : null}
        </View>
        <View style={styles.meterLine}>
          <T variant="callout" numberOfLines={1} style={styles.meterTitle} testID="step-meter-title">
            {title}
          </T>
          {activity ? (
            <T variant="mono" color={theme.mut} numberOfLines={1} style={styles.shrink} testID="step-meter-activity">
              {activity}
            </T>
          ) : null}
        </View>
      </Pressable>
      {/* Beside the body, level with its first line. */}
      {action ? <View style={styles.meterAction}>{action}</View> : null}
    </View>
  );
}

/** The steps of the plan in a sheet, opened from the meter. */
export function StepSheet({
  visible,
  onClose,
  count,
  steps,
  states,
  activity,
}: {
  visible: boolean;
  onClose(): void;
  count: string;
  steps: readonly ProgressStep[];
  states: readonly PipState[];
  activity?: string | null;
}) {
  const theme = useTheme();
  return (
    <Sheet visible={visible} onClose={onClose}>
      <T variant="title">Tasks</T>
      <T variant="mono" color={theme.mut} style={styles.tabular}>
        {count}
      </T>
      <ScrollView style={styles.stepScroll}>
        <StepList steps={steps} states={states} activity={activity} />
      </ScrollView>
      <Button tall kind="secondary" label="Close" onPress={onClose} />
    </Sheet>
  );
}

/**
 * A plan the agent stopped part way, over the composer: where it stopped, the
 * count, the pips (a tap lists the steps), the agent to continue with, and
 * Continue. Continue starts a new session; nothing happens until it is tapped.
 * `reason` says why it cannot run here, and `error` why the last tap did not.
 */
export function ResumePanel({
  heading,
  count,
  steps,
  states,
  choices,
  picked,
  onPick,
  label,
  pending,
  disabled,
  reason,
  onContinue,
}: {
  heading: string;
  count: string;
  steps: readonly ProgressStep[];
  states: readonly PipState[];
  /** The agents to continue with, the session's own first; chips show when there is a choice. */
  choices: readonly { agent: AgentType; name: string }[];
  picked: AgentType;
  onPick(agent: AgentType): void;
  label: string;
  pending: boolean;
  disabled: boolean;
  reason: string | null;
  onContinue(): void;
}) {
  const theme = useTheme();
  const [listed, setListed] = useState(false);
  return (
    <View style={[styles.resume, { backgroundColor: theme.surface }]} testID="resume-panel">
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={`${heading}. ${count}. ${listed ? "Hide" : "Show"} tasks`}
        accessibilityState={{ expanded: listed }}
        onPress={() => setListed((v) => !v)}
        style={({ pressed }) => [styles.resumeHead, { opacity: pressed ? PRESSED_OPACITY : 1 }]}
        testID="resume-steps"
      >
        <View style={styles.meterLine}>
          <View style={[styles.stopMark, { borderColor: theme.accent }]} />
          <T style={[styles.shrink, styles.resumeTitle]} numberOfLines={listed ? undefined : 1} testID="resume-title">
            {heading}
          </T>
        </View>
        <View style={styles.meterLine}>
          <Pips states={states} />
          <T variant="mono" color={theme.mut} style={styles.tabular} testID="resume-count">
            {count}
          </T>
        </View>
      </Pressable>
      {listed ? (
        <ScrollView style={styles.stepScroll}>
          <StepList steps={steps} states={states} />
        </ScrollView>
      ) : null}
      {choices.length > 1 ? (
        <ScrollView horizontal showsHorizontalScrollIndicator={false} contentContainerStyle={styles.chips} testID="resume-agents">
          {choices.map((c) => {
            const on = c.agent === picked;
            return (
              <Pressable
                key={c.agent}
                accessibilityRole="radio"
                accessibilityLabel={c.name}
                accessibilityState={{ selected: on, disabled: pending }}
                disabled={pending}
                onPress={() => onPick(c.agent)}
                hitSlop={{ top: 4, bottom: 4 }}
                style={({ pressed }) => [
                  styles.agentChip,
                  { borderColor: on ? theme.accent : theme.lineStrong, opacity: pressed ? PRESSED_OPACITY : 1 },
                ]}
                testID={`resume-agent-${c.agent}`}
              >
                <T variant="callout" color={on ? theme.accent : theme.fg}>
                  {c.name}
                </T>
              </Pressable>
            );
          })}
        </ScrollView>
      ) : null}
      <Button tall kind="primary" label={label} loading={pending} disabled={disabled} onPress={onContinue} testID="resume-continue" />
      {reason ? (
        <T variant="callout" color={theme.mut} testID="resume-reason">
          {reason}
        </T>
      ) : (
        <T variant="callout" color={theme.mut} testID="resume-note">
          {CONTINUE_NOTE}
        </T>
      )}
    </View>
  );
}

/**
 * The first message of a session that finishes another's plan: the long
 * prompt the Host wrote is not shown. This card stands in for it, with the
 * files it carried and a way back to where the plan came from. The message's
 * own actions stay under it.
 */
export function ContinuedCard({
  agentName,
  completed,
  total,
  files,
  from,
  onOpenFile,
  onOpenFrom,
  revert,
}: {
  agentName: string | null;
  completed?: number;
  total?: number;
  files: readonly AttachmentBlock[];
  /** The old session's title, or null for none; `onOpenFrom` is absent when the store no longer lists it. */
  from: string | null;
  onOpenFile?(file: AttachmentBlock): void;
  onOpenFrom?(): void;
  revert?: ReactNode;
}) {
  const theme = useTheme();
  const head = continuingLabel(agentName, completed, total);
  return (
    <View style={styles.rowGap}>
      <View style={[styles.continued, { backgroundColor: theme.surface }]} testID="continued-card">
        <T style={styles.resumeTitle} selectable>
          {head}
        </T>
        {files.length > 0 ? (
          <View style={styles.continuedFiles}>
            {files.map((f) => (
              <SentFileChip key={f.attachment_id} file={f} onPress={onOpenFile ? () => onOpenFile(f) : undefined} />
            ))}
          </View>
        ) : null}
        {onOpenFrom ? (
          <Pressable
            accessibilityRole="link"
            accessibilityLabel={fromLabel(from)}
            onPress={onOpenFrom}
            style={({ pressed }) => [styles.linkRow, { opacity: pressed ? PRESSED_OPACITY : 1 }]}
            testID="continued-from"
          >
            <T variant="callout" color={theme.accent}>
              {fromLabel(from)}
            </T>
          </Pressable>
        ) : (
          <T variant="callout" color={theme.mut} testID="continued-from">
            {fromLabel(from)}
          </T>
        )}
      </View>
      {revert}
    </View>
  );
}

/** Where this session's plan went, in place of the resume panel. */
export function ContinuedInRow({ title, onOpen }: { title: string | null | undefined; onOpen(): void }) {
  const theme = useTheme();
  return (
    <Pressable
      accessibilityRole="link"
      accessibilityLabel={continuedInLabel(title)}
      onPress={onOpen}
      style={({ pressed }) => [styles.continuedIn, { opacity: pressed ? PRESSED_OPACITY : 1 }]}
      testID="continued-in"
    >
      <T variant="callout" color={theme.mut} numberOfLines={1}>
        {continuedInLabel(title)}
      </T>
    </Pressable>
  );
}

/**
 * The message box: multiline input over a row of the session's controls and
 * the round accent send. An accent border marks an unattended session.
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
  transcript,
  onOpenTranscript,
  onRemoveTranscript,
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
  /** A fork's transcript: goes with the next message, or as the message. */
  transcript?: AttachmentBlock | null;
  onOpenTranscript?(): void;
  onRemoveTranscript?(): void;
}) {
  const theme = useTheme();
  const can = !disabled && !sending && (value.trim().length > 0 || files.length > 0 || !!transcript);
  return (
    <View
      style={[
        styles.composer,
        { backgroundColor: theme.surface, borderColor: border ?? "transparent", opacity: disabled ? 0.6 : 1 },
      ]}
      testID="composer"
    >
      <PendingFiles
        transcript={
          transcript ? (
            <TranscriptChip
              file={transcript}
              disabled={disabled || sending}
              onOpen={onOpenTranscript}
              onRemove={() => onRemoveTranscript?.()}
            />
          ) : null
        }
        files={files}
        errors={fileErrors}
        disabled={sending}
        onRemove={(id) => onRemoveFile?.(id)}
      />
      <TextInput
        value={value}
        onChangeText={onChange}
        placeholder={placeholder}
        placeholderTextColor={theme.faint}
        multiline
        editable={!disabled && !sending}
        style={[type.body, styles.input, { color: theme.fg }]}
        accessibilityLabel="Message"
        testID="composer-input"
      />
      {note ? (
        <T variant="monoSm" color={theme.mut} numberOfLines={1} style={styles.composerNote} testID="composer-note">
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
            hitSlop={{ top: 4, bottom: 4, left: 8, right: 8 }}
            style={({ pressed }) => [styles.attach, { opacity: disabled || sending ? 0.4 : pressed ? PRESSED_OPACITY : 1 }]}
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
            { backgroundColor: can || sending ? theme.accent : theme.raised, opacity: pressed ? PRESSED_OPACITY : 1 },
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

/** 32pt mono rows reach the 44pt target through their slop, keeping the transcript dense. */
const ROW_SLOP = { top: 6, bottom: 6 };
/** "Reset to here" is a 22pt row (18pt callout, 2pt padding); its slop makes the 44pt target. */
const REVERT_SLOP = { top: 11, bottom: 11, left: 8, right: 8 };

const styles = StyleSheet.create({
  shrink: { flexShrink: 1 },
  flexSpacer: { flex: 1 },
  tabular: { fontVariant: ["tabular-nums"] },
  rowGap: { gap: space.xs },
  revert: { alignSelf: "flex-end", flexDirection: "row", alignItems: "center", gap: space.xxs, paddingVertical: space.xxs },
  checkRow: { flexDirection: "row", alignItems: "center", gap: space.sm, minHeight: 44 },
  user: {
    alignSelf: "flex-end",
    maxWidth: "85%",
    borderTopLeftRadius: radius.xl,
    borderTopRightRadius: radius.xl,
    borderBottomRightRadius: space.xs,
    borderBottomLeftRadius: radius.xl,
    paddingHorizontal: space.md,
    paddingVertical: space.sm,
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
  subagent: { borderLeftWidth: 2, paddingLeft: space.md, marginLeft: space.xs, marginTop: space.xs, gap: space.sm },
  prompt: { borderRadius: radius.md, padding: space.md },
  output: { borderRadius: radius.md, flexGrow: 0 },
  outputContent: { padding: space.md },
  sheetBody: { gap: space.md },
  error: { borderRadius: radius.lg, borderLeftWidth: 3, padding: space.md, gap: space.xs },
  // Left-aligned text, a full touch target tall.
  retryButton: { minHeight: RETRY_TARGET, flexDirection: "row", alignItems: "center", gap: space.sm, alignSelf: "flex-start" },
  retryLabel: { fontFamily: fonts.sansMedium },
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
  // One row that never wraps; `pipWindow` keeps it short, the clip is the backstop.
  pips: { flexDirection: "row", flexWrap: "nowrap", alignItems: "center", gap: space.xs, flexShrink: 1, overflow: "hidden" },
  pip: { width: 8, height: 8, borderRadius: radius.full, flexShrink: 0 },
  steps: { gap: space.xs },
  step: { minHeight: 32, flexDirection: "row", alignItems: "flex-start", gap: space.sm, paddingTop: space.xs },
  stepHere: { fontFamily: fonts.sansMedium },
  stepScroll: { flexGrow: 0, maxHeight: 220 },
  // 64pt: 4 above, a 32pt first line (level with Stop), 2, a 22pt second line, 4 below.
  meter: { height: 64, flexDirection: "row", alignItems: "flex-start", paddingLeft: space.lg, paddingRight: space.sm, gap: space.sm },
  meterBody: { flex: 1, minWidth: 0, height: 64, paddingVertical: space.xs, gap: space.xxs },
  meterTop: { height: 32, flexDirection: "row", alignItems: "center", gap: space.sm },
  meterAction: { marginTop: space.xs, height: 32, justifyContent: "center" },
  meterTitle: { flexShrink: 0, maxWidth: "60%" },
  meterLine: { flexDirection: "row", alignItems: "center", gap: space.sm },
  resume: { marginHorizontal: space.md, borderRadius: radius.xl, padding: space.md, gap: space.sm },
  resumeHead: { minHeight: 44, justifyContent: "center", gap: space.xs },
  resumeTitle: { fontFamily: fonts.sansMedium },
  stopMark: { width: 10, height: 10, borderRadius: radius.full, borderWidth: 1.5 },
  chips: { gap: space.sm },
  agentChip: {
    height: 36,
    paddingHorizontal: space.md,
    borderRadius: radius.full,
    borderWidth: 1,
    alignItems: "center",
    justifyContent: "center",
  },
  continued: { borderRadius: radius.xl, padding: space.md, gap: space.sm },
  continuedFiles: { flexDirection: "row", flexWrap: "wrap", gap: space.xs },
  linkRow: { minHeight: 44, justifyContent: "center", alignSelf: "flex-start" },
  forkStart: { textAlign: "center", paddingVertical: space.xl },
  forkCard: { gap: space.xxs, borderRadius: radius.xl, paddingHorizontal: space.lg, paddingVertical: space.md, margin: space.md },
  transcriptOpen: { flexDirection: "row", alignItems: "center", gap: space.xs, flexShrink: 1 },
  forkIcon: { minWidth: 44, minHeight: 44, justifyContent: "center" },
  continuedIn: { minHeight: 44, justifyContent: "center", paddingHorizontal: space.lg },
  composer: {
    marginHorizontal: space.md,
    marginTop: space.sm,
    borderRadius: radius.xl,
    borderWidth: 1,
    paddingLeft: space.md,
    paddingRight: space.sm,
    paddingTop: space.xs,
    paddingBottom: space.sm,
    gap: space.xs,
  },
  input: { minHeight: 40, maxHeight: 140, paddingTop: space.sm, paddingBottom: space.xs, paddingRight: space.sm, fontFamily: fonts.sans },
  composerNote: { paddingBottom: space.xxs },
  composerRow: { flexDirection: "row", alignItems: "center", gap: space.sm },
  composerControls: { flex: 1, minWidth: 0, flexDirection: "row", alignItems: "center", gap: space.xs },
  send: { width: 36, height: 36, borderRadius: radius.full, alignItems: "center", justifyContent: "center" },
  attach: { width: 28, height: 36, alignItems: "center", justifyContent: "center" },
  sentFiles: { alignSelf: "flex-end", maxWidth: "85%", flexDirection: "row", flexWrap: "wrap", justifyContent: "flex-end", gap: space.xs },
  pendingFiles: { flexDirection: "row", flexWrap: "wrap", gap: space.xs, paddingTop: space.xs, paddingRight: space.sm },
  pendingChip: { borderWidth: 1 },
});
