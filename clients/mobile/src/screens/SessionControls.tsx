// A session's controls (DESIGN.md, Screen 5): one chip inside the composer for
// the mode, model and effort (⚡ when fast), a ring for the context in use
// (opening the session menu with its figures), and the Agent &
// model sheet the chip opens: the agent, the models with a search, and the
// Effort and Mode switches. Every value is the Host's; a change is sent on its
// own between turns and the sheet shows what the Host reports back. Context
// figures are the agent's own, and one it didn't report reads "not reported".

import { useRef, useState, type ReactNode } from "react";
import { Pressable, ScrollView, StyleSheet, Switch, TextInput, useWindowDimensions, View } from "react-native";
import Svg, { Circle } from "react-native-svg";
import { hasCapability, type SessionView } from "@zwiim/sdk";
import {
  changeRequest,
  contextChip,
  contextFigures,
  contextLine,
  contextSummary,
  needsConfirm,
  optionChips,
  optionsEditable,
  optionsError,
  type ContextRow,
  type OptionChange,
} from "../sessions/controls";
import { agentLock, heatLook, levelAt, modeSegments, showAllLabel, visibleModels } from "../sessions/picker";
import { agentNote, choiceLabel, offeredAgents } from "../sessions/start";
import { useCurrentHost, useHostState } from "../app/hooks";
import { currentHost } from "../services";
import { Button, Sheet, T } from "../ui/components";
import { Bot, Check, ChevronDown, ChevronRight, ChevronUp, Icon, ListChecks, Search, Shield, ShieldOff, X, Zap } from "../ui/icons";
import { SlideSwitch } from "../ui/slide-switch";
import { PRESSED_OPACITY, radius, space, type, useTheme, type Palette } from "../ui/tokens";

const MODE_ICON = { normal: Shield, plan: ListChecks, unattended: ShieldOff } as const;

/** The options chip and the context ring inside the composer, left of send. Each opens the Session options sheet. */
export function ComposerChips({
  view,
  locked,
  onOpen,
  onContext,
}: {
  view: SessionView | undefined;
  /** A turn runs: the options wait for it, so the chip dims. */
  locked: boolean;
  /** The chip: Agent & model. */
  onOpen(): void;
  /** The ring: the session menu, with the context figures. */
  onContext(): void;
}) {
  const theme = useTheme();
  const chips = optionChips(view?.options ?? null);
  const unattended = chips.mode?.mode === "unattended";
  const ink = unattended ? theme.needInk : theme.mut;
  const context = contextChip(view);
  const fast = view?.options?.fast === true;
  return (
    <>
      {chips.text ? (
        <Chip
          onPress={onOpen}
          label={`Agent and model: ${chips.text}${fast ? ", fast" : ""}`}
          fill={unattended ? theme.need : theme.raised}
          shrink
          dim={locked}
          testID="chip-options"
        >
          <Icon icon={Bot} color={ink} size={14} />
          <T variant="monoSm" color={ink} numberOfLines={1} style={unattended ? [styles.shrink, styles.strong] : styles.shrink}>
            {chips.text}
          </T>
          {fast ? <Icon icon={Zap} color={unattended ? theme.needInk : theme.accent} size={12} /> : null}
          <Icon icon={ChevronDown} color={unattended ? theme.needInk : theme.faint} size={12} />
        </Chip>
      ) : null}
      <View style={styles.flex} />
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={`Context: ${context === "—" ? "not reported" : `${context} in use`}`}
        onPress={onContext}
        hitSlop={8}
        style={({ pressed }) => [styles.ring, pressed && { opacity: 0.7 }]}
        testID="chip-context"
      >
        <Ring fraction={contextFigures(view).fraction} theme={theme} />
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
 * Agent & model: one page with the agent row, the model list with its
 * search, and the Effort and Mode switches (and Fast where the model has
 * it). Each pick goes to the Host at once and the sheet stays open; on a
 * refusal the control returns to what the Host holds and the error says why.
 */
export function SessionOptionsSheet({
  visible,
  onClose,
  session_id,
  agent,
  agentType,
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
  /** The session's agent, to mark it in the agent row. */
  agentType?: string | null;
  view: SessionView | undefined;
  connected: boolean;
  running: boolean;
  hostName: string;
}) {
  const theme = useTheme();
  const { height } = useWindowDimensions();
  const state = useHostState();
  const compat = useCurrentHost().view?.compat ?? null;
  const options = view?.options ?? null;
  const [confirmUnattended, setConfirmUnattended] = useState(false);
  const [changing, setChanging] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [expanded, setExpanded] = useState(false);
  const editable = optionsEditable({ options, connected, running, changing, hostName });

  const close = () => {
    setConfirmUnattended(false);
    setError(null);
    setQuery("");
    setExpanded(false);
    onClose();
  };

  const run = async (send: () => Promise<unknown>) => {
    setConfirmUnattended(false);
    setChanging(true);
    setError(null);
    try {
      await send();
    } catch (e) {
      setError(optionsError(e, hostName));
    } finally {
      setChanging(false);
    }
  };
  const change = (c: OptionChange) => run(() => currentHost.setSessionOptions(session_id, changeRequest(c)));

  const pick = (current: string | boolean, c: OptionChange, value: string | boolean) => {
    if (value === current || !editable.ok) return;
    if (needsConfirm(c)) return setConfirmUnattended(true);
    void change(c);
  };

  // A fork can still change agent until its first message.
  const sent = !(view?.history?.length === 0 && !view.settled && !view.live.userMessage && !view.live.message && !running);
  const lock = agentLock({ canSetAgent: hasCapability(compat, "session.set_agent"), forkedFrom: !!view?.forkedFrom, sent });
  const agents = offeredAgents(state.agents) ?? [];

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

  if (confirmUnattended) {
    return (
      <Sheet visible={visible} onClose={close}>
        <T variant="title">Turn on Auto?</T>
        <T color={theme.mut}>The agent will run commands and edit files in this workspace without asking you first.</T>
        <View style={styles.confirm}>
          <Button kind="secondary" label="Keep asking" onPress={() => setConfirmUnattended(false)} />
          <Button
            kind="destructive"
            label="Turn on Auto"
            onPress={() => void change({ mode: "unattended" })}
            testID="options-unattended-confirm"
          />
        </View>
      </Sheet>
    );
  }

  const model = options?.model ?? null;
  const effort = options?.effort ?? null;
  const modes = modeSegments(options?.mode ?? null);
  const fast = hasCapability(compat, "session.fast") ? options?.fast ?? null : null;
  const list = model ? visibleModels(model.values, { expanded, query }) : null;
  const more = model && !query.trim() ? showAllLabel(model.values.length, expanded) : null;
  const effortAt = effort ? levelAt(effort.values, effort.current) : null;
  const modeAt = Math.max(0, modes.findIndex((m) => m.mode === options?.mode?.current));
  const locked = !editable.ok;

  return (
    <Sheet visible={visible} onClose={close}>
      <ScrollView
        style={{ maxHeight: Math.round(height * 0.8) }}
        contentContainerStyle={styles.content}
        keyboardShouldPersistTaps="handled"
        testID="options-root"
      >
        <T variant="title">Agent &amp; model</T>
        {notes}
        {lock === "switchable" && agents.length > 1 ? (
          <ScrollView
            horizontal
            showsHorizontalScrollIndicator={false}
            snapToInterval={AGENT_CHIP + space.sm}
            decelerationRate="fast"
            contentContainerStyle={styles.agents}
            testID="options-agents"
          >
            {agents.map((a) => {
              const note = agentNote(a);
              const current = a.agent === agentType;
              const can = a.available && !locked && !current;
              return (
                <Pressable
                  key={a.agent}
                  accessibilityRole="button"
                  accessibilityLabel={note ? `${a.name}, ${note}` : a.name}
                  accessibilityState={{ selected: current, disabled: !can }}
                  disabled={!can}
                  onPress={() => {
                    // The new agent's models: the old search and expansion don't apply.
                    setQuery("");
                    setExpanded(false);
                    void run(() => currentHost.setSessionAgent(session_id, a.agent, {}));
                  }}
                  style={({ pressed }) => [
                    styles.agentChip,
                    {
                      backgroundColor: theme.surface,
                      borderColor: current ? theme.accent : "transparent",
                      opacity: !a.available ? 0.45 : pressed ? PRESSED_OPACITY : 1,
                    },
                  ]}
                  testID={`options-agent-${a.agent}`}
                >
                  <Icon icon={Bot} color={current ? theme.accent : theme.mut} size={16} />
                  <View style={styles.shrink}>
                    <T variant="callout" numberOfLines={1}>
                      {a.name}
                    </T>
                    {note ? (
                      <T variant="monoSm" color={theme.faint} numberOfLines={1}>
                        {note}
                      </T>
                    ) : null}
                  </View>
                </Pressable>
              );
            })}
          </ScrollView>
        ) : agent ? (
          <View style={[styles.agentLocked, { backgroundColor: theme.surface }]} testID="options-agent">
            <Icon icon={Bot} color={theme.mut} size={16} />
            <T variant="callout" numberOfLines={1} style={styles.shrink}>
              {agent}
            </T>
            <T variant="monoSm" color={theme.faint} numberOfLines={1}>
              set at first message
            </T>
          </View>
        ) : null}

        {model && list ? (
          <View style={styles.section}>
            <View style={styles.searchRow}>
              <View style={[styles.search, styles.flex, { backgroundColor: theme.surface }]}>
                <Icon icon={Search} color={theme.faint} size={16} />
                <TextInput
                  value={query}
                  onChangeText={setQuery}
                  placeholder={agent ? `Search ${agent} models` : "Search models"}
                  placeholderTextColor={theme.faint}
                  autoCapitalize="none"
                  autoCorrect={false}
                  spellCheck={false}
                  returnKeyType="search"
                  style={[type.body, styles.searchInput, { color: theme.fg }]}
                  accessibilityLabel="Search models"
                  testID="options-model-search"
                />
                {query ? (
                  <Pressable accessibilityRole="button" accessibilityLabel="Clear search" onPress={() => setQuery("")} hitSlop={10} testID="options-model-clear">
                    <Icon icon={X} color={theme.faint} size={16} />
                  </Pressable>
                ) : null}
              </View>
              {more ? (
                <Pressable
                  accessibilityRole="button"
                  onPress={() => setExpanded((e) => !e)}
                  hitSlop={8}
                  style={styles.more}
                  testID="options-model-more"
                >
                  <T variant="callout" color={theme.accent}>
                    {more}
                  </T>
                </Pressable>
              ) : null}
            </View>
            {list.shown.length > 0 ? (
              list.shown.map((v) => {
                const current = v.value === model.current;
                return (
                  <Pressable
                    key={v.value}
                    accessibilityRole="button"
                    accessibilityState={{ selected: current, disabled: locked }}
                    disabled={locked}
                    onPress={() => pick(model.current, { model: v.value }, v.value)}
                    style={({ pressed }) => [styles.modelRow, { opacity: locked ? 0.5 : pressed ? PRESSED_OPACITY : 1 }]}
                    testID={`options-model-${v.value}`}
                  >
                    <T variant="body" numberOfLines={1} style={styles.modelName}>
                      {v.label}
                    </T>
                    <T variant="callout" color={theme.faint} numberOfLines={1} style={[styles.shrink, styles.flex]}>
                      {v.description ?? ""}
                    </T>
                    {current ? <Icon icon={Check} color={theme.accent} size={16} /> : null}
                  </Pressable>
                );
              })
            ) : (
              <T variant="callout" color={theme.mut} testID="options-model-empty">
                No models match "{query.trim()}"
              </T>
            )}
          </View>
        ) : null}

        {effort && effort.values.length > 1 ? (
          <View style={styles.section}>
            <View style={styles.summary}>
              <T variant="caption" color={theme.mut}>
                Effort
              </T>
              <T
                variant="callout"
                color={effortAt === null ? theme.mut : heatLook(effortAt, effort.values.length).text}
                numberOfLines={1}
                style={styles.shrink}
              >
                {choiceLabel(effort, effort.current)}
              </T>
            </View>
            <SlideSwitch
              segments={effort.values.map((v) => ({ key: v.value, label: v.label }))}
              value={effortAt}
              onChange={(i) => pick(effort.current, { effort: effort.values[i]!.value }, effort.values[i]!.value)}
              tone="heat"
              disabled={locked}
              testID="options-effort"
            />
          </View>
        ) : null}

        {modes.length > 0 ? (
          <View style={styles.section}>
            <T variant="caption" color={theme.mut}>
              Mode
            </T>
            <SlideSwitch
              segments={modes.map((m) => ({
                key: m.mode,
                label: m.label,
                enabled: m.enabled,
                icon: <Icon icon={MODE_ICON[m.mode]} color={m.mode === options?.mode?.current ? (m.mode === "unattended" ? theme.needInk : theme.fg) : theme.faint} size={14} />,
              }))}
              value={modeAt}
              onChange={(i) => pick(options!.mode!.current, { mode: modes[i]!.mode }, modes[i]!.mode)}
              tone="plain"
              accentAt={2}
              disabled={locked}
              testID="options-mode"
            />
            <T variant="callout" color={options?.mode?.current === "unattended" ? theme.need : theme.mut} testID="options-mode-hint">
              {modes[modeAt]?.hint}
            </T>
          </View>
        ) : null}

        {fast !== null ? (
          <View style={[styles.fastRow, { backgroundColor: theme.surface }]}>
            <Icon icon={Zap} color={fast ? theme.accent : theme.mut} size={16} />
            <T variant="body" style={styles.flex}>
              Fast
            </T>
            <Switch
              value={fast}
              disabled={locked}
              onValueChange={(on) => pick(fast, { fast: on }, on)}
              trackColor={{ true: theme.accent, false: theme.raised }}
              accessibilityLabel="Fast"
              testID="options-fast"
            />
          </View>
        ) : null}
      </ScrollView>
    </Sheet>
  );
}

/**
 * The header's `⋯`: what this session is (title; agent · workspace · host),
 * its context in use, opening the figures, and Stop agent while it runs.
 * Stop agent opens its own confirm once this sheet has slid away.
 */
export function SessionMenuSheet({
  visible,
  onClose,
  title,
  detail,
  view,
  agent,
  live,
  connected,
  onStopAgent,
}: {
  visible: boolean;
  onClose(): void;
  title: string;
  /** Agent · workspace · host, in one mono line. */
  detail: string;
  view: SessionView | undefined;
  agent: string | null;
  live: boolean;
  connected: boolean;
  onStopAgent(): void;
}) {
  const theme = useTheme();
  const [context, setContext] = useState(false);
  const stopNext = useRef(false);
  const close = () => {
    setContext(false);
    onClose();
  };
  return (
    <Sheet
      visible={visible}
      onClose={close}
      onClosed={() => {
        if (!stopNext.current) return;
        stopNext.current = false;
        onStopAgent();
      }}
    >
      <View style={styles.head}>
        <T variant="title" numberOfLines={2} testID="menu-title">
          {title}
        </T>
        <T variant="monoSm" color={theme.mut} numberOfLines={1} testID="menu-detail">
          {detail}
        </T>
      </View>
      {context ? (
        <Context view={view} agent={agent} />
      ) : (
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={`Context: ${contextLine(view)}`}
          onPress={() => setContext(true)}
          style={({ pressed }) => [styles.contextLine, pressed && { opacity: 0.7 }]}
          testID="menu-context-line"
        >
          <T variant="monoSm" color={theme.mut} style={[styles.tabular, styles.shrink]} numberOfLines={1}>
            {contextLine(view)}
          </T>
          <Icon icon={ChevronRight} color={theme.faint} size={14} />
        </Pressable>
      )}
      {live ? (
        <Button
          kind="destructive"
          label="Stop agent"
          disabled={!connected}
          onPress={() => {
            stopNext.current = true;
            close();
          }}
          testID="session-stop-agent"
        />
      ) : null}
    </Sheet>
  );
}

/** An agent chip's width in the agent row, which snaps chip by chip. */
const AGENT_CHIP = 148;

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
    height: 30,
    borderRadius: 15,
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
  agents: { gap: space.sm },
  agentChip: { width: AGENT_CHIP, height: 44, borderRadius: radius.md, borderWidth: 1, paddingHorizontal: space.sm, flexDirection: "row", alignItems: "center", gap: space.sm },
  agentLocked: { height: 44, borderRadius: radius.md, paddingHorizontal: space.md, flexDirection: "row", alignItems: "center", gap: space.sm },
  searchRow: { flexDirection: "row", alignItems: "center", gap: space.md },
  more: { minHeight: 40, justifyContent: "center" },
  modelRow: { minHeight: 44, flexDirection: "row", alignItems: "center", gap: space.sm },
  modelName: { flexShrink: 0, maxWidth: "60%" },
  fastRow: { height: 48, borderRadius: radius.md, paddingHorizontal: space.md, flexDirection: "row", alignItems: "center", gap: space.sm },
  fold: { minHeight: 40, flexDirection: "row", alignItems: "center", gap: space.sm },
  figure: { minHeight: 40, paddingHorizontal: space.md, flexDirection: "row", alignItems: "center", gap: space.sm },
});
