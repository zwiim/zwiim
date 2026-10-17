import { useEffect, useState } from "react";
import { Copy, Loader2 } from "lucide-react";
import type { AgentRequest, PermissionTool, RequestAnswer } from "@zwiim/sdk";
import { Markdown } from "@/components/Markdown";
import { Button } from "@/components/ui/button";
import { errorMessage } from "@/lib/host-context";
import { cn } from "@/lib/utils";

/**
 * A pending permission / question / plan-approval request. Answers go to the
 * host; the card disappears when the host reports the request resolved.
 */
export function RequestCard({
  request,
  onAnswer,
  disabled = false,
  handOff = false,
}: {
  request: AgentRequest;
  onAnswer(answer: RequestAnswer): Promise<void>;
  /** Offline: the request stays visible but cannot be answered. */
  disabled?: boolean;
  /** The Host can continue a plan in a new session (`session.plan_hand_off`). */
  handOff?: boolean;
}) {
  /** The answer being sent: its button shows a spinner. */
  const [sending, setSending] = useState<string | null>(null);
  const busy = sending !== null || disabled;
  const [error, setError] = useState<string | null>(null);
  const isPlan = request.kind === "permission" && (!!request.plan_markdown || request.tool?.kind === "switch_mode");

  async function submit(answer: RequestAnswer, which: string) {
    setSending(which);
    setError(null);
    try {
      await onAnswer(answer);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setSending(null);
    }
  }

  return (
    <div
      // "Needs you": the accent rule on the card's leading edge.
      className="bg-card animate-enter flex flex-col gap-3 rounded-lg p-4 shadow-[inset_2px_0_0_var(--brand)] motion-reduce:animate-none"
      data-testid="request-card"
    >
      {request.kind === "permission" && (
        <>
          <p className="font-medium">{isPlan ? "The agent has a plan" : "Permission requested"}</p>
          {/* A plan already in the transcript is only asked about here. */}
          {request.title && !request.plan_markdown && (
            <p className={cn("text-muted-foreground text-xs", !isPlan && "font-mono break-all")}>{request.title}</p>
          )}
          {request.plan_markdown ? (
            <PlanText text={request.plan_markdown} />
          ) : (
            request.tool && <ToolDetails tool={request.tool} />
          )}
          <div className="flex flex-wrap gap-2">
            {request.options.map((o) => (
              <Button
                key={o.option_id}
                size="sm"
                variant={o.kind.startsWith("allow") ? "brand" : "outline"}
                disabled={busy}
                onClick={() => submit({ kind: "permission", answer: { option_id: o.option_id } }, o.option_id)}
              >
                {sending === o.option_id && <Loader2 className="animate-spin" />}
                {o.name}
              </Button>
            ))}
          </div>
        </>
      )}
      {request.kind === "question" && (
        <QuestionForm request={request} busy={busy} sending={sending} onSubmit={submit} />
      )}
      {request.kind === "plan_approval" && (
        <PlanForm request={request} busy={busy} sending={sending} handOff={handOff} onSubmit={submit} />
      )}
      {error && <p className="text-destructive text-xs">{error}</p>}
    </div>
  );
}

/** A spinner on the button whose answer is being sent. */
function Sending({ on }: { on: boolean }) {
  return on ? <Loader2 className="animate-spin" /> : null;
}

function QuestionForm({
  request,
  busy,
  sending,
  onSubmit,
}: {
  request: Extract<AgentRequest, { kind: "question" }>;
  busy: boolean;
  sending: string | null;
  onSubmit(a: RequestAnswer, which: string): void;
}) {
  const [selected, setSelected] = useState<Record<string, string[]>>({});
  const [other, setOther] = useState<Record<string, string>>({});

  const toggle = (qid: string, label: string, multi: boolean) =>
    setSelected((s) => {
      const cur = s[qid] ?? [];
      const next = multi
        ? cur.includes(label)
          ? cur.filter((l) => l !== label)
          : [...cur, label]
        : [label];
      return { ...s, [qid]: next };
    });

  const answers = request.questions.map((q) => ({
    question_id: q.question_id,
    labels: [...(selected[q.question_id] ?? []), ...(other[q.question_id]?.trim() ? [other[q.question_id]!.trim()] : [])],
  }));

  return (
    <>
      <p className="font-medium">The agent has a question</p>
      {request.questions.map((q) => (
        <div key={q.question_id} className="flex flex-col gap-2">
          {q.header && (
            <span className="text-muted-foreground text-2xs font-semibold tracking-wide uppercase">{q.header}</span>
          )}
          <p>{q.question}</p>
          <div className="flex flex-col gap-1">
            {q.options.map((o) => {
              const on = selected[q.question_id]?.includes(o.label) ?? false;
              return (
                <button
                  key={o.label}
                  type="button"
                  aria-pressed={on}
                  className={cn(
                    "ease-std rounded-md border px-3 py-2 text-left transition-colors duration-120",
                    on ? "border-brand bg-brand-soft" : "hover:bg-muted",
                  )}
                  onClick={() => toggle(q.question_id, o.label, q.multi_select)}
                >
                  <span className="font-medium">{o.label}</span>
                  {o.description && <span className="text-muted-foreground block text-xs">{o.description}</span>}
                </button>
              );
            })}
            <input
              className="bg-background rounded-md border px-3 py-2"
              placeholder="Other…"
              value={other[q.question_id] ?? ""}
              onChange={(e) => setOther((s) => ({ ...s, [q.question_id]: e.target.value }))}
            />
          </div>
        </div>
      ))}
      <div className="flex gap-2">
        <Button
          size="sm"
          variant="brand"
          disabled={busy || answers.some((a) => a.labels.length === 0)}
          onClick={() => onSubmit({ kind: "question", answer: { answers } }, "answer")}
        >
          <Sending on={sending === "answer"} />
          Answer
        </Button>
        <Button
          size="sm"
          variant="outline"
          disabled={busy}
          onClick={() => onSubmit({ kind: "question", answer: { answers: [], declined: true } }, "decline")}
        >
          <Sending on={sending === "decline"} />
          Decline
        </Button>
      </div>
    </>
  );
}

function PlanForm({
  request,
  busy,
  sending,
  handOff,
  onSubmit,
}: {
  request: Extract<AgentRequest, { kind: "plan_approval" }>;
  busy: boolean;
  sending: string | null;
  handOff: boolean;
  onSubmit(a: RequestAnswer, which: string): void;
}) {
  const [feedback, setFeedback] = useState("");
  const [confirmAbandon, setConfirmAbandon] = useState(false);
  const [copied, setCopied] = useState(false);
  const adjustments = feedback.trim();
  const plan = (answer: Extract<RequestAnswer, { kind: "plan_approval" }>["answer"]) =>
    onSubmit({ kind: "plan_approval", answer }, answer.decision);

  useEffect(() => {
    if (!copied) return;
    const t = setTimeout(() => setCopied(false), 1500);
    return () => clearTimeout(t);
  }, [copied]);

  return (
    <>
      <p className="font-medium">Review the plan</p>
      <PlanText text={request.plan_markdown} />
      <textarea
        className="bg-background min-h-16 rounded-md border px-3 py-2"
        placeholder="Enter your plan adjustments here…"
        value={feedback}
        onChange={(e) => setFeedback(e.target.value)}
      />
      <div className="flex flex-wrap items-center gap-2">
        {confirmAbandon ? (
          <span className="flex items-center gap-1.5 text-xs" data-testid="confirm-abandon">
            Discard this plan?
            <Button size="sm" variant="destructive" disabled={busy} onClick={() => plan({ decision: "abandon" })}>
              <Sending on={sending === "abandon"} />
              Discard
            </Button>
            <Button size="sm" variant="ghost" disabled={busy} onClick={() => setConfirmAbandon(false)}>
              Keep
            </Button>
          </span>
        ) : (
          <Button
            size="sm"
            variant="ghost"
            className="text-destructive enabled:hover:text-destructive"
            disabled={busy}
            onClick={() => setConfirmAbandon(true)}
          >
            Abandon
          </Button>
        )}
        <span className="flex-1" />
        <Button
          size="sm"
          variant="ghost"
          onClick={() => {
            void navigator.clipboard.writeText(request.plan_markdown).then(() => setCopied(true));
          }}
        >
          <Copy />
          {copied ? "Copied" : "Copy"}
        </Button>
        {handOff && (
          <Button
            size="sm"
            variant="outline"
            disabled={busy}
            title="Continue this plan in a new session; this one stops"
            onClick={() => plan({ decision: "hand_off" })}
          >
            <Sending on={sending === "hand_off"} />
            Hand off
          </Button>
        )}
        <Button
          size="sm"
          variant="brand"
          disabled={busy}
          onClick={() =>
            plan(adjustments ? { decision: "request_changes", feedback: adjustments } : { decision: "approve" })
          }
        >
          <Sending on={sending === "approve" || sending === "request_changes"} />
          {adjustments ? "Send adjustments" : "Approve"}
        </Button>
      </div>
    </>
  );
}

function PlanText({ text }: { text: string }) {
  return (
    <div className="bg-background/60 max-h-96 overflow-auto rounded-lg p-3" data-testid="plan">
      <Markdown text={text} />
    </div>
  );
}

/** What a permission would let the agent do: the command, the edits, or its description. */
function ToolDetails({ tool }: { tool: PermissionTool }) {
  return (
    <div className="flex flex-col gap-2" data-testid="permission-tool">
      {tool.command && (
        <pre className="bg-code overflow-x-auto rounded-lg px-3 py-2 font-mono text-xs whitespace-pre-wrap">
          $ {tool.command}
        </pre>
      )}
      {tool.diffs.map((d, i) => (
        <div key={i} className="bg-code overflow-hidden rounded-lg text-xs">
          <div className="text-muted-foreground border-b px-3 py-1.5 font-mono">{d.path ?? "file"}</div>
          <pre className="max-h-60 overflow-auto font-mono leading-5">
            {(d.old_text ?? "").split("\n").filter((l, j, a) => l !== "" || j < a.length - 1).map((l, j) => (
              <div key={`o${j}`} className="bg-del px-2">
                <span className="text-destructive select-none">-</span>
                {l}
              </div>
            ))}
            {(d.new_text ?? "").split("\n").filter((l, j, a) => l !== "" || j < a.length - 1).map((l, j) => (
              <div key={`n${j}`} className="bg-add px-2">
                <span className="text-done select-none">+</span>
                {l}
              </div>
            ))}
          </pre>
        </div>
      ))}
      {!tool.command && tool.diffs.length === 0 && tool.text && (
        <p className="text-muted-foreground text-xs whitespace-pre-wrap">{tool.text}</p>
      )}
    </div>
  );
}
