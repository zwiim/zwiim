// A session's controls (DESIGN.md, Screen 5): one chip inside the composer for
// the mode, model and effort, a ring for the context in use, and the Session
// options sheet they open (also from the header's `⋯`): three summary rows, each
// opening its own page to pick from, and a context line opening the figures.
// Every value is the Host's; a change is sent on its own between turns and the
// sheet shows what the Host reports back. Context figures are the agent's own,
// and one it didn't report reads "not reported".

import { useEffect, useState, type ReactNode } from "react";
import { Pressable, ScrollView, StyleSheet, TextInput, useWindowDimensions, View } from "react-native";
import Svg, { Circle } from "react-native-svg";
import type { SessionMode, SessionOptions, SessionView } from "@zwiim/sdk";
import {
  changeRequest,
  contextChip,
  contextFigures,
  contextLine,
  contextSummary,
  filterChoices,
  needsConfirm,
  optionChips,
  optionRows,
  optionsEditable,
  optionsError,
  SEARCH_FROM,
  settleOptions,
  type ContextRow,
  type OptionChange,
  type OptionsPage,
} from "../sessions/controls";
import { MODE_LABELS } from "../sessions/start";
import { currentHost } from "../services";
import { Field, RadioGroup } from "../ui/choices";
import { Button, Sheet, T } from "../ui/components";
import { ChevronDown, ChevronLeft, ChevronRight, ChevronUp, Icon, ListChecks, Search, Shield, ShieldOff } from "../ui/icons";
import { PRESSED_OPACITY, radius, space, type, useTheme, type Palette } from "../ui/tokens";

const MODE_ICON = { normal: Shield, plan: ListChecks, unattended: ShieldOff } as const;

/** The options chip and the context ring inside the composer, left of send. Each opens the Session options sheet. */
export function ComposerChips({
  view,
  locked,
  onOpen,
}: {
  view: SessionView | undefined;
  /** A turn runs: the options wait for it, so the chip dims. */
  locked: boolean;
  onOpen(): void;
}) {
  const theme = useTheme();
  const chips = optionChips(view?.options ?? null);
  const mode = chips.mode;
  const unattended = mode?.mode === "unattended";
  const ink = unattended ? theme.needInk : theme.mut;
  const context = contextChip(view);
  return (
    <>
      {chips.text ? (
        <Chip
          onPress={onOpen}
          label={`Session options: ${chips.text}`}
          fill={unattended ? theme.need : theme.raised}
          shrink
          dim={locked}
          testID="chip-options"
        >
          {mode ? <Icon icon={MODE_ICON[mode.mode]} color={ink} size={14} /> : null}
          <T variant="monoSm" color={ink} numberOfLines={1} style={unattended ? [styles.shrink, styles.strong] : styles.shrink}>
            {chips.text}
          </T>
          <Icon icon={ChevronDown} color={unattended ? theme.needInk : theme.faint} size={12} />
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
        { backgroundColor: fill, opacity: pressed ? PRESSED_OPACITY : dim ? 0.55 : 1 },
      ]}
      testID={testID}
    >
      {children}
    </Pressable>
  );
}

/**
 * Session options: the agent, a summary row for each of mode / model / effort it
 * offers, and the context line. A row opens its page to pick from; a pick goes
 * back to the summary. Changes go one at a time, between turns.
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
  const { height } = useWindowDimensions();
  const options = view?.options ?? null;
  const [confirmUnattended, setConfirmUnattended] = useState(false);
  const [changing, setChanging] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [page, setPage] = useState<OptionsPage>("root");
  const [query, setQuery] = useState("");
  const editable = optionsEditable({ options, connected, running, changing, hostName });
  const rows = optionRows(options, editable.ok);
  // A page whose option locks (a turn starts, the connection drops) falls back to
  // the summary, and stays there, search cleared, once the option unlocks.
  const settled = settleOptions({ page, query }, rows);
  const shown = settled.page;
  useEffect(() => {
    if (settled.page === page) return;
    setPage(settled.page);
    setQuery(settled.query);
  }, [settled.page, settled.query, page]);

  const open = (next: OptionsPage) => {
    setQuery("");
    setPage(next);
  };

  const close = () => {
    setConfirmUnattended(false);
    setError(null);
    open("root");
    onClose();
  };

  const change = async (c: OptionChange) => {
    setConfirmUnattended(false);
    open("root");
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

  const pick = (current: string, c: OptionChange, value: string) => {
    if (value === current) return open("root");
    if (needsConfirm(c)) return setConfirmUnattended(true);
    void change(c);
  };

  const notes = (
    <>
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
    </>
  );

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
  } else if (shown === "root") {
    body = (
      <ScrollView contentContainerStyle={styles.content} testID="options-root">
        <View style={styles.head}>
          <T variant="title">Session options</T>
          {agent ? (
            <T variant="monoSm" color={theme.mut} testID="options-agent">
              {agent}
            </T>
          ) : null}
        </View>
        {notes}
        {rows.length > 0 ? (
          <View style={[styles.group, { backgroundColor: theme.surface }]}>
            {rows.map((r, i) => (
              <Field
                key={r.key}
                first={i === 0}
                label={r.label}
                value={r.value}
                color={r.key === "mode" && options?.mode?.current === "unattended" ? theme.need : undefined}
                onPress={r.open ? () => open(r.key) : undefined}
                testID={`options-row-${r.key}`}
              />
            ))}
          </View>
        ) : null}
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
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={`Context: ${contextLine(view)}`}
          onPress={() => open("context")}
          style={({ pressed }) => [styles.contextLine, pressed && { opacity: 0.7 }]}
          testID="options-context-line"
        >
          <T variant="monoSm" color={theme.mut} style={[styles.tabular, styles.shrink]} numberOfLines={1}>
            {contextLine(view)}
          </T>
          <Icon icon={ChevronRight} color={theme.faint} size={14} />
        </Pressable>
      </ScrollView>
    );
  } else {
    const title = shown === "context" ? "Context" : rows.find((r) => r.key === shown)?.label ?? "";
    body = (
      <>
        <View style={styles.pageHead}>
          <Pressable
            accessibilityRole="button"
            accessibilityLabel="Back"
            onPress={() => open("root")}
            hitSlop={8}
            style={({ pressed }) => [styles.back, pressed && { opacity: 0.7 }]}
            testID="options-back"
          >
            <Icon icon={ChevronLeft} color={theme.accent} size={22} />
          </Pressable>
          <View style={styles.pageTitle}>
            <T variant="headline" numberOfLines={1}>
              {title}
            </T>
            {agent ? (
              <T variant="monoSm" color={theme.mut} numberOfLines={1}>
                {agent}
              </T>
            ) : null}
          </View>
          <View style={styles.back} />
        </View>
        {notes}
        <ScrollView
          style={{ maxHeight: Math.round(height * 0.62) }}
          contentContainerStyle={styles.content}
          keyboardShouldPersistTaps="handled"
        >
          {shown === "context" ? (
            <Context view={view} agent={agent} />
          ) : (
            <OptionPage options={options} page={shown} query={query} onQuery={setQuery} onPick={pick} theme={theme} />
          )}
        </ScrollView>
      </>
    );
  }

  return (
    <Sheet visible={visible} onClose={close}>
      {body}
    </Sheet>
  );
}

/** One option's values to pick from; the model list gets a search once it is long. */
function OptionPage({
  options,
  page,
  query,
  onQuery,
  onPick,
  theme,
}: {
  options: SessionOptions | null;
  page: "mode" | "model" | "effort";
  query: string;
  onQuery(q: string): void;
  onPick(current: string, c: OptionChange, value: string): void;
  theme: Palette;
}) {
  if (page === "mode") {
    const mode = options?.mode;
    if (!mode) return null;
    return (
      <RadioGroup
        items={mode.available.map((m) => ({
          value: m,
          label: MODE_LABELS[m].label,
          detail: MODE_LABELS[m].hint,
          icon: MODE_ICON[m],
          tint: m === "unattended" ? theme.need : undefined,
        }))}
        current={mode.current}
        onChoose={(m) => onPick(mode.current, { mode: m as SessionMode }, m)}
        testID="options-mode"
      />
    );
  }
  if (page === "effort") {
    const effort = options?.effort;
    if (!effort) return null;
    return (
      <RadioGroup
        items={effort.values.map((v) => ({ value: v.value, label: v.label, detail: v.description }))}
        current={effort.current}
        onChoose={(v) => onPick(effort.current, { effort: v }, v)}
        testID="options-effort"
      />
    );
  }
  const model = options?.model;
  if (!model) return null;
  const found = filterChoices(model.values, query);
  return (
    <>
      {model.values.length >= SEARCH_FROM ? (
        <View style={[styles.search, { backgroundColor: theme.surface }]}>
          <Icon icon={Search} color={theme.faint} size={16} />
          <TextInput
            value={query}
            onChangeText={onQuery}
            placeholder="Search models"
            placeholderTextColor={theme.faint}
            autoCapitalize="none"
            autoCorrect={false}
            spellCheck={false}
            returnKeyType="search"
            style={[type.body, styles.searchInput, { color: theme.fg }]}
            accessibilityLabel="Search models"
            testID="options-model-search"
          />
        </View>
      ) : null}
      {found.length > 0 ? (
        <RadioGroup
          items={found.map((v) => ({ value: v.value, label: v.label, detail: v.description }))}
          current={model.current}
          onChoose={(v) => onPick(model.current, { model: v }, v)}
          testID="options-model"
        />
      ) : (
        <T variant="callout" color={theme.mut} testID="options-model-empty">
          No models match "{query.trim()}"
        </T>
      )}
    </>
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
            color={theme.mut}
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
      <T variant="callout" color={theme.mut}>
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
          <T variant="mono" color={r.value === null ? theme.mut : theme.fg} style={styles.tabular}>
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
    paddingHorizontal: space.sm,
    flexDirection: "row",
    alignItems: "center",
    gap: space.xs,
  },
  ring: { height: 28, flexDirection: "row", alignItems: "center", gap: space.xs, paddingRight: space.xxs },
  content: { gap: space.md },
  head: { gap: space.xxs },
  pageHead: { flexDirection: "row", alignItems: "center", gap: space.sm },
  pageTitle: { flex: 1, alignItems: "center", gap: space.xxs },
  back: { width: 44, height: 44, justifyContent: "center" },
  contextLine: { minHeight: 44, flexDirection: "row", alignItems: "center", gap: space.xs },
  search: { height: 40, borderRadius: radius.md, paddingHorizontal: space.md, flexDirection: "row", alignItems: "center", gap: space.sm },
  searchInput: { flex: 1, padding: 0 },
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
