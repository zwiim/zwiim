// What an agent asks for (DESIGN.md, Screen 4 cards and the Request sheet):
// a NEEDS YOU card that resolves a command's permission right there, and a
// sheet for everything that needs reading first (changes, plans, questions).
// Answers go to the host with `session.respond`; the card leaves when the
// host resolves the request on the session's stream, not before.

import { useEffect, useState } from "react";
import { ScrollView, StyleSheet, TextInput, View, Pressable, useWindowDimensions } from "react-native";
import * as Clipboard from "expo-clipboard";
import * as Haptics from "expo-haptics";
import { hasCapability, type AgentRequest, type PermissionOption, type RequestAnswer, type ToolDiff } from "@zwiim/sdk";
import { useCurrentHost } from "../app/hooks";
import { useRootNavigation } from "../app/navigation";
import {
  diffCounts,
  diffLines,
  elapsed,
  isPrimaryOption,
  isRejectOption,
  orderOptions,
  questionAnswer,
  questionsAnswered,
  requestAction,
  requestTitle,
  respondError,
  togglePick,
  type SessionItem,
} from "../sessions/model";
import { currentHost } from "../services";
import { Button, CommandBlock, Sheet, T } from "../ui/components";
import { Check, Copy } from "../ui/icons";
import { fonts, radius, space, useTheme } from "../ui/tokens";

/**
 * Sending one answer to a request. Once sent, `sending` stays set until the
 * request is gone: the host's resolution, not the reply, updates the screen.
 */
function useRespond(session_id: string, request_id: string, hostName: string) {
  const nav = useRootNavigation();
  const [sending, setSending] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    setSending(null);
    setError(null);
  }, [request_id]);
  const send = async (answer: RequestAnswer, key: string): Promise<boolean> => {
    setSending(key);
    setError(null);
    try {
      const handedOff = await currentHost.respond(session_id, request_id, answer);
      // A plan handed off: its new session takes the old one's place on screen.
      if (handedOff) {
        const top = nav.getState()?.routes.at(-1);
        if (top?.name === "Session") nav.replace("Session", { session_id: handedOff, started: true });
        else nav.navigate("Session", { session_id: handedOff, started: true });
      }
      return true;
    } catch (e) {
      setSending(null);
      setError(respondError(e, hostName));
      return false;
    }
  };
  return { sending, error, send };
}

const permission = (o: PermissionOption): RequestAnswer => ({ kind: "permission", answer: { option_id: o.option_id } });

function tapFeedback(o: PermissionOption) {
  if (!isRejectOption(o)) void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Light);
}

/** A NEEDS YOU card: whose request, what it asks, and how to answer. */
export function RequestCard({
  item,
  hostName,
  now,
  showTitle = true,
}: {
  item: SessionItem & { request: AgentRequest };
  hostName: string;
  now: number;
  /** The session's title; off inside the session itself. */
  showTitle?: boolean;
}) {
  const theme = useTheme();
  const { request } = item;
  const action = requestAction(request);
  const { sending, error, send } = useRespond(item.session_id, request.request_id, hostName);
  const [reviewing, setReviewing] = useState(false);
  const meta = [item.agent, item.workspace].filter(Boolean).join(" · ");
  return (
    <View style={[styles.card, { backgroundColor: theme.surface, borderLeftColor: theme.need }]} testID={`request-${item.session_id}`}>
      <View style={styles.cardMeta}>
        <T variant="monoSm" color={theme.mut} numberOfLines={1} style={styles.flex}>
          {meta || "agent"}
        </T>
        <T variant="monoSm" color={theme.need} style={styles.tabular}>
          {item.waitingSince ? `waiting ${elapsed(item.waitingSince, now)}` : "waiting"}
        </T>
      </View>
      {showTitle ? (
        <T numberOfLines={2} style={styles.cardTitle}>
          {item.title}
        </T>
      ) : null}
      <T variant="callout" color={theme.mut} numberOfLines={3} testID="request-title">
        {requestTitle(request)}
      </T>
      {action.type === "inline" ? (
        <>
          <CommandBlock text={action.command} testID="request-command" />
          <View style={styles.inlineActions}>
            {action.options.map((o) => {
              const primary = isPrimaryOption(o, request.kind === "permission" ? request.options : []);
              return (
                <Button
                  key={o.option_id}
                  testID={`option-${o.kind}`}
                  label={o.name}
                  kind={primary ? "need" : isRejectOption(o) ? "ghost" : "secondary"}
                  loading={sending === o.option_id}
                  disabled={sending !== null && sending !== o.option_id}
                  onPress={() => {
                    tapFeedback(o);
                    void send(permission(o), o.option_id);
                  }}
                />
              );
            })}
          </View>
        </>
      ) : (
        <Button testID="request-review" tall kind="need" label={action.label} onPress={() => setReviewing(true)} />
      )}
      {error ? (
        <T variant="callout" color={theme.err} testID="request-error">
          {error}
        </T>
      ) : null}
      {action.type === "review" ? (
        <RequestSheet
          visible={reviewing}
          onClose={() => setReviewing(false)}
          session_id={item.session_id}
          request={request}
          hostName={hostName}
        />
      ) : null}
    </View>
  );
}

/** The request sheet: read what the agent asks, then answer. */
export function RequestSheet({
  visible,
  onClose,
  session_id,
  request,
  hostName,
}: {
  visible: boolean;
  onClose(): void;
  session_id: string;
  request: AgentRequest;
  hostName: string;
}) {
  const theme = useTheme();
  const { height } = useWindowDimensions();
  const { sending, error, send } = useRespond(session_id, request.request_id, hostName);
  const answer = async (a: RequestAnswer, key: string) => {
    if (await send(a, key)) onClose();
  };
  return (
    <Sheet visible={visible} onClose={onClose}>
      <T variant="title" numberOfLines={3}>
        {requestTitle(request)}
      </T>
      <ScrollView
        style={[styles.sheetScroll, { maxHeight: height * 0.55 }]}
        contentContainerStyle={styles.sheetBody}
        keyboardShouldPersistTaps="handled"
      >
        {request.kind === "permission" ? (
          <PermissionBody request={request} />
        ) : request.kind === "plan_approval" ? (
          <PlanText markdown={request.plan_markdown} />
        ) : (
          <QuestionBody request={request} sending={sending} onAnswer={(a) => void answer(a, "answer")} />
        )}
      </ScrollView>
      {request.kind === "permission" ? (
        <View style={styles.stack}>
          {orderOptions(request.options).map((o) => (
            <Button
              key={o.option_id}
              testID={`sheet-option-${o.kind}`}
              tall
              label={o.name}
              kind={isPrimaryOption(o, request.options) ? "need" : isRejectOption(o) ? "ghost" : "secondary"}
              loading={sending === o.option_id}
              disabled={sending !== null && sending !== o.option_id}
              onPress={() => {
                tapFeedback(o);
                void answer(permission(o), o.option_id);
              }}
            />
          ))}
        </View>
      ) : request.kind === "plan_approval" ? (
        <PlanActions
          markdown={request.plan_markdown}
          sending={sending}
          onAnswer={(a, key) => void answer(a, key)}
        />
      ) : null}
      {error ? (
        <T variant="callout" color={theme.err} testID="sheet-error">
          {error}
        </T>
      ) : null}
    </Sheet>
  );
}

function PermissionBody({ request }: { request: Extract<AgentRequest, { kind: "permission" }> }) {
  const theme = useTheme();
  const tool = request.tool;
  return (
    <>
      {request.plan_markdown ? <PlanText markdown={request.plan_markdown} /> : null}
      {tool?.command ? <CommandBlock text={tool.command} /> : null}
      {tool?.diffs.map((d, i) => <DiffBlock key={`${d.path}-${i}`} diff={d} />)}
      {!tool?.command && !tool?.diffs.length && tool?.text ? (
        <T color={theme.mut} selectable>
          {tool.text}
        </T>
      ) : null}
    </>
  );
}

/** One proposed file change: header with counts, then the changed lines. */
function DiffBlock({ diff }: { diff: ToolDiff }) {
  const theme = useTheme();
  const lines = diffLines(diff.old_text, diff.new_text);
  const { added, removed } = diffCounts(lines);
  return (
    <View style={styles.diff}>
      <View style={styles.diffHeader}>
        <T variant="mono" numberOfLines={1} style={styles.flex}>
          {diff.path ?? "file"}
        </T>
        <T variant="monoSm" color={theme.done}>
          +{added}
        </T>
        <T variant="monoSm" color={theme.err}>
          −{removed}
        </T>
      </View>
      <ScrollView horizontal style={[styles.diffBody, { backgroundColor: theme.code }]} contentContainerStyle={styles.diffContent}>
        <View>
          {lines.map((l, i) => (
            <T
              key={i}
              variant="mono"
              color={l.sign === "+" ? theme.done : l.sign === "-" ? theme.err : theme.mut}
            >
              {`${l.sign === "-" ? "−" : l.sign} ${l.text}`}
            </T>
          ))}
        </View>
      </ScrollView>
    </View>
  );
}

/** The agent's plan, as written. Markdown is shown as its text. */
function PlanText({ markdown }: { markdown: string }) {
  const theme = useTheme();
  return (
    <View style={[styles.plan, { backgroundColor: theme.surface }]}>
      <T selectable testID="request-plan">
        {markdown.trim()}
      </T>
    </View>
  );
}

function PlanActions({
  markdown,
  sending,
  onAnswer,
}: {
  markdown: string;
  sending: string | null;
  onAnswer(a: RequestAnswer, key: string): void;
}) {
  const theme = useTheme();
  const { view } = useCurrentHost();
  const canHandOff = hasCapability(view?.compat ?? null, "session.plan_hand_off");
  const [feedback, setFeedback] = useState("");
  const [abandoning, setAbandoning] = useState(false);
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const t = setTimeout(() => setCopied(false), 1500);
    return () => clearTimeout(t);
  }, [copied]);
  const adjustments = feedback.trim();
  const plan = (decision: "approve" | "request_changes" | "abandon" | "hand_off", text?: string): RequestAnswer => ({
    kind: "plan_approval",
    answer: text ? { decision, feedback: text } : { decision },
  });
  const busy = sending !== null;
  const primary = adjustments ? "request_changes" : "approve";
  return (
    <View style={styles.stack}>
      <TextInput
        testID="plan-feedback"
        value={feedback}
        onChangeText={setFeedback}
        placeholder="Enter your plan adjustments here…"
        placeholderTextColor={theme.faint}
        multiline
        style={[styles.input, { backgroundColor: theme.surface, color: theme.fg }]}
      />
      <Button
        testID="plan-copy"
        tall
        kind="ghost"
        icon={copied ? Check : Copy}
        label={copied ? "Copied" : "Copy plan"}
        onPress={() => void Clipboard.setStringAsync(markdown).then(() => setCopied(true))}
      />
      {canHandOff ? (
        <Button
          testID="plan-hand-off"
          tall
          kind="secondary"
          label="Hand off to a new session"
          loading={sending === "hand_off"}
          disabled={busy && sending !== "hand_off"}
          onPress={() => onAnswer(plan("hand_off"), "hand_off")}
        />
      ) : null}
      <Button
        testID="plan-abandon"
        tall
        kind="destructive"
        label={abandoning ? "Abandon the plan? Tap again" : "Abandon"}
        loading={sending === "abandon"}
        disabled={busy && sending !== "abandon"}
        onPress={() => (abandoning ? onAnswer(plan("abandon"), "abandon") : setAbandoning(true))}
      />
      <Button
        testID="plan-approve"
        tall
        kind="need"
        label={adjustments ? "Send adjustments" : "Approve"}
        loading={sending === primary}
        disabled={busy && sending !== primary}
        onPress={() => {
          void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Light);
          onAnswer(plan(primary, adjustments || undefined), primary);
        }}
      />
    </View>
  );
}

function QuestionBody({
  request,
  sending,
  onAnswer,
}: {
  request: Extract<AgentRequest, { kind: "question" }>;
  sending: string | null;
  onAnswer(a: RequestAnswer): void;
}) {
  const theme = useTheme();
  const [picked, setPicked] = useState<Record<string, string[]>>({});
  const { questions } = request;
  const busy = sending !== null;
  return (
    <View style={styles.stack}>
      {questions.map((q) => (
        <View key={q.question_id} style={styles.question}>
          {q.header ? (
            <T variant="caption" color={theme.mut}>
              {q.header}
            </T>
          ) : null}
          {questions.length > 1 ? <T variant="headline">{q.question}</T> : null}
          {q.options.map((o) => {
            const on = picked[q.question_id]?.includes(o.label) ?? false;
            return (
              <Pressable
                key={o.label}
                testID={`answer-${o.label}`}
                accessibilityRole={q.multi_select ? "checkbox" : "radio"}
                accessibilityState={{ checked: on }}
                onPress={() =>
                  setPicked((p) => ({ ...p, [q.question_id]: togglePick(q, p[q.question_id] ?? [], o.label) }))
                }
                style={[styles.choice, { backgroundColor: on ? theme.surface : "transparent", borderColor: on ? theme.need : theme.line }]}
              >
                <T variant="mono" color={on ? theme.need : theme.mut}>
                  {q.multi_select ? (on ? "[x]" : "[ ]") : on ? "(•)" : "( )"}
                </T>
                <View style={styles.flex}>
                  <T style={styles.choiceLabel}>{o.label}</T>
                  {o.description ? (
                    <T variant="callout" color={theme.mut}>
                      {o.description}
                    </T>
                  ) : null}
                </View>
              </Pressable>
            );
          })}
        </View>
      ))}
      <Button
        testID="question-send"
        tall
        kind="need"
        label="Send answer"
        loading={sending === "answer"}
        disabled={!questionsAnswered(questions, picked) || busy}
        onPress={() => onAnswer(questionAnswer(questions, picked))}
      />
      <Button
        testID="question-decline"
        tall
        kind="ghost"
        label="Decline"
        disabled={busy}
        onPress={() => onAnswer(questionAnswer(questions, picked, true))}
      />
    </View>
  );
}

const styles = StyleSheet.create({
  flex: { flex: 1 },
  tabular: { fontVariant: ["tabular-nums"] },
  card: {
    marginHorizontal: space.md,
    marginBottom: space.sm,
    borderRadius: radius.lg,
    borderLeftWidth: 3,
    padding: space.md,
    gap: space.sm,
  },
  cardMeta: { flexDirection: "row", alignItems: "center", gap: space.sm },
  cardTitle: { fontFamily: fonts.sansSemi },
  inlineActions: { flexDirection: "row", flexWrap: "wrap", justifyContent: "flex-end", gap: space.sm },
  sheetScroll: { flexShrink: 1 },
  sheetBody: { gap: space.md },
  stack: { gap: space.sm },
  diff: { gap: space.xs },
  diffHeader: { flexDirection: "row", alignItems: "center", gap: space.sm },
  diffBody: { borderRadius: radius.md, flexGrow: 0 },
  diffContent: { padding: space.md },
  plan: { borderRadius: radius.md, padding: space.md },
  input: { minHeight: 88, borderRadius: radius.md, padding: space.md, fontFamily: fonts.sans, fontSize: 15, textAlignVertical: "top" },
  question: { gap: space.sm },
  choice: { flexDirection: "row", gap: space.md, borderWidth: 1, borderRadius: radius.md, padding: space.md, minHeight: 52 },
  choiceLabel: { fontFamily: fonts.sansMedium },
});
