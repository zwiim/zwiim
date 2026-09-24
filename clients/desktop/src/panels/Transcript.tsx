import { Check, ChevronRight, CircleDashed, Loader2, X } from "lucide-react";
import type { HistoryBlock, HistoryTurn, LiveTurn, ToolCall } from "@zwiim/sdk";
import { Markdown } from "@/components/Markdown";
import { cn } from "@/lib/utils";

export function UserBubble({ children }: { children: React.ReactNode }) {
  return (
    <div className="bg-muted max-w-[85%] self-end rounded-2xl rounded-br-md px-4 py-2.5 leading-relaxed whitespace-pre-wrap">
      {children}
    </div>
  );
}

const STATUS_LABEL: Record<string, string> = {
  pending: "pending",
  in_progress: "running",
  completed: "done",
  failed: "failed",
};

function StatusIcon({ status }: { status: string | null }) {
  if (status === "completed") return <Check className="size-3.5 text-emerald-500" />;
  if (status === "failed") return <X className="text-destructive size-3.5" />;
  if (status === "in_progress") return <Loader2 className="text-muted-foreground size-3.5 animate-spin" />;
  return <CircleDashed className="text-muted-foreground size-3.5" />;
}

/** A unified diff (`--- a` / `+++ b` / ±lines), as the host sends edits. */
const looksLikeDiff = (text: string) => text.startsWith("--- ") && text.includes("\n+++ ");

function Output({ text, error }: { text: string; error?: boolean }) {
  const diff = !error && looksLikeDiff(text);
  return (
    <pre
      className={cn(
        "max-h-72 overflow-auto rounded-lg p-3 font-mono text-xs leading-5 whitespace-pre-wrap",
        error ? "text-destructive bg-destructive/5" : "bg-code",
      )}
    >
      {diff
        ? text.split("\n").map((line, i) => (
            <div
              key={i}
              className={cn(
                line.startsWith("+") && !line.startsWith("+++") && "bg-emerald-500/10 text-emerald-800 dark:text-emerald-300",
                line.startsWith("-") && !line.startsWith("---") && "bg-red-500/10 text-red-800 dark:text-red-300",
              )}
            >
              {line || " "}
            </div>
          ))
        : text}
    </pre>
  );
}

/** One tool call: status and title; its input and output on expand. */
export function ToolCallView(props: {
  title: string | null;
  kind?: string | null;
  status: string | null;
  input?: string | null;
  output?: string | null;
  isError?: boolean;
}) {
  const failed = props.status === "failed" || props.isError;
  const hasDetails = Boolean(props.input || props.output);
  return (
    <details className="group text-xs" data-testid="tool-call" data-status={props.status ?? ""}>
      <summary
        className={cn(
          "text-muted-foreground -mx-2 flex list-none items-center gap-2 rounded-md px-2 py-1 select-none",
          hasDetails ? "hover:bg-muted/60 hover:text-foreground cursor-pointer" : "cursor-default",
        )}
        onClick={(e) => !hasDetails && e.preventDefault()}
      >
        <ChevronRight
          className={cn("size-3 shrink-0 transition-transform group-open:rotate-90", !hasDetails && "invisible")}
        />
        <StatusIcon status={failed ? "failed" : props.status} />
        <span className="text-foreground/90 truncate font-mono">{props.title || "Tool call"}</span>
        {props.kind && props.kind !== "other" && <span className="shrink-0">{props.kind}</span>}
        {props.status && (
          <span className={cn("ml-auto shrink-0", failed && "text-destructive")}>
            {failed ? "failed" : (STATUS_LABEL[props.status] ?? props.status)}
          </span>
        )}
      </summary>
      {hasDetails && (
        <div className="mt-1 mb-2 ml-5 flex flex-col gap-2">
          {props.input && <Output text={props.input} />}
          {props.output && <Output text={props.output} error={failed} />}
        </div>
      )}
    </details>
  );
}

export function Thought({ text }: { text: string }) {
  return (
    <details className="group text-muted-foreground text-xs" data-testid="thought">
      <summary className="hover:text-foreground flex w-fit cursor-pointer list-none items-center gap-1.5 select-none">
        <ChevronRight className="size-3 transition-transform group-open:rotate-90" />
        <span className="italic">Thinking</span>
      </summary>
      <div className="border-border mt-1.5 ml-1.5 border-l-2 pl-3 leading-relaxed whitespace-pre-wrap">{text}</div>
    </details>
  );
}

const userText = (blocks: { type: string; text?: string }[]) =>
  blocks
    .map((b) => (b.type === "text" ? (b.text ?? "") : ""))
    .join("")
    .trim();

export function HistoryTurnView({ turn }: { turn: HistoryTurn }) {
  if (turn.role === "user") {
    const text = userText(turn.blocks);
    return text ? <UserBubble>{text}</UserBubble> : null;
  }
  // A result is shown with its call; one without a call stands alone.
  const results = new Map<string, Extract<HistoryBlock, { type: "tool_result" }>>();
  for (const b of turn.blocks) if (b.type === "tool_result" && b.tool_call_id) results.set(b.tool_call_id, b);
  const called = new Set(
    turn.blocks.flatMap((b) => (b.type === "tool_call" && b.tool_call_id ? [b.tool_call_id] : [])),
  );
  return (
    <div className="flex flex-col gap-2.5 leading-relaxed">
      {turn.blocks.map((b, i) => {
        switch (b.type) {
          case "text":
            return <Markdown key={i} text={b.text} />;
          case "thought":
            return <Thought key={i} text={b.text} />;
          case "tool_call": {
            const r = b.tool_call_id ? results.get(b.tool_call_id) : undefined;
            return (
              <ToolCallView
                key={i}
                title={b.title}
                // Agents' transcripts often omit the status; a result means it ran.
                status={r?.is_error ? "failed" : (b.status ?? (r ? "completed" : null))}
                input={b.input_preview}
                output={r?.output_preview}
                isError={r?.is_error}
              />
            );
          }
          case "tool_result":
            if (b.tool_call_id && called.has(b.tool_call_id)) return null;
            return b.is_error && b.output_preview ? <Output key={i} text={b.output_preview} error /> : null;
          case "image":
            return (
              <img key={i} className="max-w-sm rounded-lg" src={`data:${b.mime_type};base64,${b.data}`} alt="" />
            );
          default:
            return null;
        }
      })}
    </div>
  );
}

export function LiveTurnView({ turn, streaming }: { turn: LiveTurn; streaming?: boolean }) {
  const text = turn.userMessage ? userText(turn.userMessage.blocks) : "";
  const toolCall = (id: string): ToolCall | undefined => turn.toolCalls[id];
  if (!turn.userMessage && !turn.message && !streaming) return null;
  return (
    <div className="flex flex-col gap-2.5 leading-relaxed" data-testid="live-turn">
      {text && <UserBubble>{text}</UserBubble>}
      {turn.message?.content.map((b, i) => {
        if (b.type === "text") return <Markdown key={i} text={b.text} />;
        if (b.type === "thought") return <Thought key={i} text={b.text} />;
        const tc = toolCall(b.tool_call_id);
        return (
          <ToolCallView
            key={i}
            title={tc?.title ?? null}
            kind={tc?.kind}
            status={tc?.status ?? null}
            output={typeof tc?.content === "string" ? tc.content : null}
          />
        );
      })}
      {streaming && (
        <span className="text-muted-foreground flex items-center gap-2 text-xs">
          <Loader2 className="size-3.5 animate-spin" /> Working…
        </span>
      )}
    </div>
  );
}

const STOP_REASONS: Record<string, string> = {
  cancelled: "Stopped — the turn was cancelled.",
  max_tokens: "Stopped — the reply reached the agent's length limit.",
  max_turn_requests: "Stopped — the agent reached its limit of steps for one turn.",
  refusal: "The agent declined to continue.",
};

/** Why the last turn ended, when it did not simply finish. */
export function stopReasonText(reason: string | null): string | null {
  if (reason === null || reason === "end_turn") return null;
  return STOP_REASONS[reason] ?? `Stopped (${reason.replace(/_/g, " ")}).`;
}
