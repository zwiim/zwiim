// A session's controls (DESIGN.md, Screen 5): chips inside the composer for
// the mode and the model with its effort, a ring for the context in use, and
// the Session options sheet they open (also from the header's `⋯`), which picks
// in place. Every value is the Host's; a change is sent on its own between
// turns and the sheet shows what the Host reports back. Context figures are the
// agent's own, and one it didn't report reads "not reported".

import { useState, type ReactNode } from "react";
import { Pressable, ScrollView, StyleSheet, View } from "react-native";
import Svg, { Circle } from "react-native-svg";
import type { SessionMode, SessionOptions, SessionView } from "@zwiim/sdk";
import {
  changeRequest,
  contextChip,
  contextFigures,
  contextSummary,
  optionChips,
  optionsEditable,
  optionsError,
  type ContextRow,
  type OptionChange,
} from "../sessions/controls";
import { MODE_LABELS, modeChoosable } from "../sessions/start";
import { currentHost } from "../services";
import { RadioGroup, Segmented, type Choice } from "../ui/choices";
import { Button, Sheet, T } from "../ui/components";
import { ChevronDown, ChevronUp, Icon, ListChecks, Shield, ShieldOff } from "../ui/icons";
import { radius, space, useTheme, type Palette } from "../ui/tokens";

const MODE_ICON = { normal: Shield, plan: ListChecks, unattended: ShieldOff } as const;

/** The chips inside the composer, left of send. Each opens the Session options sheet. */
export function ComposerChips({
  view,
  locked,
  onOpen,
}: {
  view: SessionView | undefined;
  /** A turn runs: the model waits for it, so its chip dims. */
  locked: boolean;
  onOpen(): void;
}) {
  const theme = useTheme();
  const chips = optionChips(view?.options ?? null);
  const mode = chips.mode;
  const unattended = mode?.mode === "unattended";
  const context = contextChip(view);
  return (
    <>
      {mode ? (
        <Chip
          onPress={onOpen}
          label={`Mode: ${mode.label}`}
          fill={unattended ? theme.need : theme.raised}
          testID="chip-mode"
        >
          <Icon icon={MODE_ICON[mode.mode]} color={unattended ? theme.needInk : theme.mut} size={14} />
          <T variant="monoSm" color={unattended ? theme.needInk : theme.mut} numberOfLines={1} style={unattended && styles.strong}>
            {mode.label}
          </T>
        </Chip>
      ) : null}
      {chips.model ? (
        <Chip onPress={onOpen} label={`Model: ${chips.model}`} fill={theme.raised} shrink dim={locked} testID="chip-model">
          <T variant="monoSm" color={theme.mut} numberOfLines={1} style={styles.shrink}>
            {chips.model}
          </T>
          <Icon icon={ChevronDown} color={theme.faint} size={12} />
        </Chip>
      ) : null}
      <View style={styles.flex} />
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={`Context: ${context === "—" ? "not reported" : `${context} in use`}`}
        onPress={onOpen}
        hitSlop={{ top: 8, bottom: 8, left: 4, right: 4 }}
        style={({ pressed }) => [styles.ring, pressed && { opacity: 0.7 }]}
        testID="chip-context"
      >
        <Ring fraction={contextFigures(view).fraction} theme={theme} />
        <T variant="monoSm" color={theme.mut} style={styles.tabular}>
          {context}
        </T>
      </Pressable>
    </>
  );
}

/** The share of the context window in use, as an arc; dashed while the agent hasn't reported it. */
function Ring({ fraction, theme }: { fraction: number | null; theme: Palette }) {
  const r = 7.5;
  const c = 2 * Math.PI * r;
  return (
    <Svg width={20} height={20} viewBox="0 0 20 20">
      {fraction === null ? (
        <Circle cx={10} cy={10} r={r} fill="none" stroke={theme.faint} strokeWidth={2} strokeDasharray="2 3" />
      ) : (
        <>
          <Circle cx={10} cy={10} r={r} fill="none" stroke={theme.faint} strokeOpacity={0.45} strokeWidth={2.5} />
          <Circle
            cx={10}
            cy={10}
            r={r}
            fill="none"
            stroke={theme.accent}
            strokeWidth={2.5}
            strokeLinecap="round"
            strokeDasharray={`${Math.max(0.02, fraction) * c} ${c}`}
            transform="rotate(-90 10 10)"
          />
        </>
      )}
    </Svg>
  );
}

function Chip({
  onPress,
  label,
  fill,
  shrink,
  dim,
  testID,
  children,
}: {
  onPress(): void;
  label: string;
  fill: string;
  shrink?: boolean;
  dim?: boolean;
  testID: string;
  children: ReactNode;
}) {
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={label}
      onPress={onPress}
      hitSlop={{ top: 8, bottom: 8 }}
      style={({ pressed }) => [
        styles.chip,
        shrink && styles.shrink,
        { backgroundColor: fill, opacity: pressed ? 0.8 : dim ? 0.55 : 1 },
      ]}
      testID={testID}
    >
      {children}
    </Pressable>
  );
}

/**
 * Session options: the agent, its mode / model / effort (only what it offers),
 * picked in place, and the context it reported. Changes go one at a time,
 * between turns.
 */
export function SessionOptionsSheet({
  visible,
  onClose,
  session_id,
  agent,
  view,
  connected,
  running,
  hostName,
}: {
  visible: boolean;
  onClose(): void;
  session_id: string;
  /** The agent's display name. */
  agent: string | null;
  view: SessionView | undefined;
  connected: boolean;
  running: boolean;
  hostName: string;
}) {
  const theme = useTheme();
  const options = view?.options ?? null;
  const [confirmUnattended, setConfirmUnattended] = useState(false);
  const [changing, setChanging] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const editable = optionsEditable({ options, connected, running, changing, hostName });

  const close = () => {
    setConfirmUnattended(false);
    setError(null);
    onClose();
  };

  const change = async (c: OptionChange) => {
    setConfirmUnattended(false);
    setChanging(true);
    setError(null);
    try {
      await currentHost.setSessionOptions(session_id, changeRequest(c));
    } catch (e) {
      setError(optionsError(e, hostName));
    } finally {
      setChanging(false);
    }
  };

  let body;
  if (confirmUnattended) {
    body = (
      <>
        <T variant="title">Turn off approvals?</T>
        <T color={theme.mut}>The agent will run commands and edit files in this workspace without asking you first.</T>
        <View style={styles.confirm}>
          <Button kind="secondary" label="Keep asking" onPress={() => setConfirmUnattended(false)} />
          <Button
            kind="destructive"
            label="Run unattended"
            onPress={() => void change({ mode: "unattended" })}
            testID="options-unattended-confirm"
          />
        </View>
      </>
    );
  } else {
    body = (
      <ScrollView contentContainerStyle={styles.content}>
        <View style={styles.head}>
          <T variant="title">Session options</T>
          {agent ? (
            <T variant="monoSm" color={theme.mut} testID="options-agent">
              {agent}
            </T>
          ) : null}
        </View>
        {!editable.ok ? (
          <T variant="callout" color={theme.mut} testID="options-hint">
            {editable.reason}
          </T>
        ) : null}
        {error ? (
          <T variant="callout" color={theme.err} testID="options-error">
            {error}
          </T>
        ) : null}
        <Pickers
          options={options}
          enabled={editable.ok}
          onChange={(c) => {
            if ("mode" in c && c.mode === "unattended") return setConfirmUnattended(true);
            void change(c);
          }}
          theme={theme}
        />
        {options?.mode?.current === "unattended" ? (
          <View style={styles.note} testID="options-unattended">
            <T variant="callout" color={theme.need}>
              Unattended: the agent runs commands and edits files without asking.
            </T>
            {options.mode.available.includes("normal") ? (
              <Button
                kind="secondary"
                label="Ask first again"
                disabled={!editable.ok}
                onPress={() => void change({ mode: "normal" })}
                testID="options-ask-again"
              />
            ) : null}
          </View>
        ) : null}
        <Context view={view} agent={agent} />
      </ScrollView>
    );
  }

  return (
    <Sheet visible={visible} onClose={close}>
      {body}
    </Sheet>
  );
}

/** Mode and Effort side by side, Model as a list, each only when the agent offers it. */
function Pickers({
  options,
  enabled,
  onChange,
  theme,
}: {
  options: SessionOptions | null;
  enabled: boolean;
  onChange(c: OptionChange): void;
  theme: Palette;
}) {
  if (!options) return null;
  const { mode, model, effort } = options;
  const pick = (current: string, c: OptionChange, value: string) => {
    if (value !== current) onChange(c);
  };
  const values = (o: NonNullable<SessionOptions["model"]>): Choice[] =>
    o.values.map((v) => ({ value: v.value, label: v.label, detail: v.description }));
  return (
    <>
      {mode ? (
        <Section title="Mode">
          <Segmented
            items={mode.available.map((m) => ({
              value: m,
              label: MODE_LABELS[m].label,
              icon: MODE_ICON[m],
              tint: m === "unattended" ? theme.need : undefined,
            }))}
            current={mode.current}
            disabled={!enabled || !modeChoosable(options)}
            onChoose={(m) => pick(mode.current, { mode: m as SessionMode }, m)}
            testID="options-mode"
          />
        </Section>
      ) : null}
      {model ? (
        <Section title="Model">
          <RadioGroup
            items={values(model)}
            current={model.current}
            disabled={!enabled || model.values.length < 2}
            onChoose={(v) => pick(model.current, { model: v }, v)}
            testID="options-model"
          />
        </Section>
      ) : null}
      {effort ? (
        <Section title="Effort">
          {effort.values.length <= 5 ? (
            <Segmented
              items={values(effort)}
              current={effort.current}
              disabled={!enabled || effort.values.length < 2}
              onChoose={(v) => pick(effort.current, { effort: v }, v)}
              testID="options-effort"
            />
          ) : (
            <RadioGroup
              items={values(effort)}
              current={effort.current}
              disabled={!enabled}
              onChoose={(v) => pick(effort.current, { effort: v }, v)}
              testID="options-effort"
            />
          )}
        </Section>
      ) : null}
    </>
  );
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  const theme = useTheme();
  return (
    <View style={styles.section}>
      <T variant="caption" color={theme.mut}>
        {title}
      </T>
      {children}
    </View>
  );
}

/** The context window in one line and the last turn's tokens folded under it, as the agent reported them. */
function Context({ view, agent }: { view: SessionView | undefined; agent: string | null }) {
  const theme = useTheme();
  const [open, setOpen] = useState(false);
  const figures = contextFigures(view);
  const summary = contextSummary(view);
  const total = figures.lastTurn?.find((r) => r.key === "total")?.value ?? null;
  return (
    <View style={styles.section} testID="options-context">
      <T variant="caption" color={theme.mut}>
        Context
      </T>
      {figures.fraction !== null ? (
        <View style={[styles.track, { backgroundColor: theme.surface }]} testID="context-bar">
          <View
            style={[styles.fill, { width: `${Math.max(2, figures.fraction * 100)}%`, backgroundColor: theme.accent }]}
          />
        </View>
      ) : null}
      {summary ? (
        <View style={styles.summary} testID="context-summary">
          <T variant="mono" style={[styles.shrink, styles.tabular]} numberOfLines={1}>
            {summary.main}
          </T>
          <T
            variant="mono"
            color={summary.aside.endsWith("not reported") ? theme.faint : theme.mut}
            style={styles.tabular}
            numberOfLines={1}
          >
            {summary.aside}
          </T>
        </View>
      ) : (
        <T variant="callout" color={theme.mut} testID="context-unreported">
          {agent ?? "The agent"} hasn't reported its context window yet.
        </T>
      )}
      {figures.lastTurn ? (
        <>
          <Pressable
            accessibilityRole="button"
            accessibilityState={{ expanded: open }}
            onPress={() => setOpen(!open)}
            style={({ pressed }) => [styles.fold, pressed && { opacity: 0.7 }]}
            testID="context-last-turn"
          >
            <T variant="callout" color={theme.mut} style={styles.flex}>
              {total === null ? "Last turn" : `Last turn · ${total} tokens`}
            </T>
            <Icon icon={open ? ChevronUp : ChevronDown} color={theme.mut} size={16} />
          </Pressable>
          {open ? <Figures rows={figures.lastTurn} /> : null}
        </>
      ) : null}
      <T variant="callout" color={theme.faint}>
        As {agent ?? "the agent"} reports them. Nothing here is estimated.
      </T>
    </View>
  );
}

function Figures({ rows }: { rows: ContextRow[] }) {
  const theme = useTheme();
  return (
    <View style={[styles.group, { backgroundColor: theme.surface }]}>
      {rows.map((r, i) => (
        <View
          key={r.key}
          style={[styles.figure, { borderTopColor: theme.line, borderTopWidth: i === 0 ? 0 : StyleSheet.hairlineWidth }]}
          testID={`context-${r.key}`}
        >
          <T color={theme.mut} style={styles.flex}>
            {r.label}
          </T>
          <T variant="mono" color={r.value === null ? theme.faint : theme.fg} style={styles.tabular}>
            {r.value ?? "not reported"}
          </T>
        </View>
      ))}
    </View>
  );
}

const styles = StyleSheet.create({
  flex: { flex: 1 },
  shrink: { flexShrink: 1 },
  strong: { fontWeight: "500" },
  tabular: { fontVariant: ["tabular-nums"] },
  chip: {
    height: 28,
    borderRadius: radius.sm,
    paddingHorizontal: space.sm + 1,
    flexDirection: "row",
    alignItems: "center",
    gap: space.xs + 1,
  },
  ring: { height: 28, flexDirection: "row", alignItems: "center", gap: space.xs + 1, paddingRight: space.xxs },
  content: { gap: space.md },
  head: { gap: space.xxs },
  group: { borderRadius: radius.lg, overflow: "hidden" },
  note: { gap: space.sm, alignItems: "flex-start" },
  confirm: { flexDirection: "row", gap: space.sm, justifyContent: "flex-end" },
  section: { gap: space.sm },
  track: { height: 6, borderRadius: radius.full, overflow: "hidden" },
  fill: { height: 6, borderRadius: radius.full },
  summary: { flexDirection: "row", alignItems: "baseline", justifyContent: "space-between", gap: space.sm },
  fold: { minHeight: 40, flexDirection: "row", alignItems: "center", gap: space.sm },
  figure: { minHeight: 40, paddingHorizontal: space.md, flexDirection: "row", alignItems: "center", gap: space.sm },
});
