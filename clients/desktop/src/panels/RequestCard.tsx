import { useState } from "react";
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
}: {
  request: AgentRequest;
  onAnswer(answer: RequestAnswer): Promise<void>;
  /** Offline: the request stays visible but cannot be answered. */
  disabled?: boolean;
}) {
  const [sending, setBusy] = useState(false);
  const busy = sending || disabled;
  const [error, setError] = useState<string | null>(null);

  async function submit(answer: RequestAnswer) {
    setBusy(true);
    setError(null);
    try {
      await onAnswer(answer);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div
      className="bg-card flex flex-col gap-3 rounded-xl border-l-2 border-l-amber-500 p-4 shadow-sm"
      data-testid="request-card"
    >
      {request.kind === "permission" && (
        <>
          <p className="font-medium">{request.plan_markdown ? "The agent has a plan" : "Permission requested"}</p>
          {request.title && !request.plan_markdown && (
            <p className="text-muted-foreground font-mono text-xs break-all">{request.title}</p>
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
                variant={o.kind.startsWith("allow") ? "default" : "outline"}
                disabled={busy}
                onClick={() => submit({ kind: "permission", answer: { option_id: o.option_id } })}
              >
                {o.name}
              </Button>
            ))}
          </div>
        </>
      )}
      {request.kind === "question" && (
        <QuestionForm request={request} busy={busy} onSubmit={submit} />
      )}
      {request.kind === "plan_approval" && (
        <PlanForm request={request} busy={busy} onSubmit={submit} />
      )}
      {error && <p className="text-destructive text-xs">{error}</p>}
    </div>
  );
}

function QuestionForm({
  request,
  busy,
  onSubmit,
}: {
  request: Extract<AgentRequest, { kind: "question" }>;
  busy: boolean;
  onSubmit(a: RequestAnswer): void;
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
          {q.header && <span className="text-muted-foreground text-xs font-medium uppercase">{q.header}</span>}
          <p>{q.question}</p>
          <div className="flex flex-col gap-1">
            {q.options.map((o) => {
              const on = selected[q.question_id]?.includes(o.label) ?? false;
              return (
                <button
                  key={o.label}
                  className={cn(
                    "rounded-md border px-2.5 py-1.5 text-left",
                    on ? "border-primary bg-primary/5" : "hover:bg-muted",
                  )}
                  onClick={() => toggle(q.question_id, o.label, q.multi_select)}
                >
                  <span className="font-medium">{o.label}</span>
                  {o.description && <span className="text-muted-foreground block text-xs">{o.description}</span>}
                </button>
              );
            })}
            <input
              className="bg-background rounded-md border px-2.5 py-1.5"
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
          disabled={busy || answers.some((a) => a.labels.length === 0)}
          onClick={() => onSubmit({ kind: "question", answer: { answers } })}
        >
          Answer
        </Button>
        <Button
          size="sm"
          variant="outline"
          disabled={busy}
          onClick={() => onSubmit({ kind: "question", answer: { answers: [], declined: true } })}
        >
          Decline
        </Button>
      </div>
    </>
  );
}

function PlanForm({
  request,
  busy,
  onSubmit,
}: {
  request: Extract<AgentRequest, { kind: "plan_approval" }>;
  busy: boolean;
  onSubmit(a: RequestAnswer): void;
}) {
  const [feedback, setFeedback] = useState("");
  const [confirmAbandon, setConfirmAbandon] = useState(false);
  return (
    <>
      <p className="font-medium">Review the plan</p>
      <PlanText text={request.plan_markdown} />
      <textarea
        className="bg-background min-h-16 rounded-md border px-2.5 py-1.5"
        placeholder="Feedback (for requesting changes)"
        value={feedback}
        onChange={(e) => setFeedback(e.target.value)}
      />
      <div className="flex gap-2">
        <Button size="sm" disabled={busy} onClick={() => onSubmit({ kind: "plan_approval", answer: { decision: "approve" } })}>
          Approve
        </Button>
        <Button
          size="sm"
          variant="outline"
          disabled={busy || !feedback.trim()}
          onClick={() =>
            onSubmit({ kind: "plan_approval", answer: { decision: "request_changes", feedback: feedback.trim() } })
          }
        >
          Request changes
        </Button>
        {confirmAbandon ? (
          <span className="flex items-center gap-1.5 text-xs" data-testid="confirm-abandon">
            Discard this plan?
            <Button
              size="sm"
              variant="destructive"
              disabled={busy}
              onClick={() => onSubmit({ kind: "plan_approval", answer: { decision: "abandon" } })}
            >
              Discard
            </Button>
            <Button size="sm" variant="ghost" disabled={busy} onClick={() => setConfirmAbandon(false)}>
              Keep
            </Button>
          </span>
        ) : (
          <Button size="sm" variant="outline" disabled={busy} onClick={() => setConfirmAbandon(true)}>
            Abandon
          </Button>
        )}
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
              <div key={`o${j}`} className="bg-red-500/10 px-2 text-red-800 dark:text-red-300">
                -{l}
              </div>
            ))}
            {(d.new_text ?? "").split("\n").filter((l, j, a) => l !== "" || j < a.length - 1).map((l, j) => (
              <div key={`n${j}`} className="bg-emerald-500/10 px-2 text-emerald-800 dark:text-emerald-300">
                +{l}
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
