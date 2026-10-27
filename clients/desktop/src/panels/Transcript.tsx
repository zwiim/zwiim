import { createContext, Fragment, memo, useCallback, useContext, useEffect, useId, useRef, useState } from "react";
import { createPortal } from "react-dom";
import {
  ArrowRightLeft,
  Bot,
  Brain,
  ChevronRight,
  FileCode2,
  FilePen,
  FilePlus2,
  FileText,
  FolderOpen,
  Globe,
  Image,
  Keyboard,
  Layers,
  ListCollapse,
  ListTodo,
  MessageCircleQuestion,
  RotateCcw,
  Search,
  Terminal,
  Trash2,
  Wrench,
  X,
} from "lucide-react";
import {
  clockTime,
  firstSeen,
  historySubagentCards,
  isLaunch,
  lateSubagentBlocks,
  launchFirstSeen,
  liveSubagentCards,
  modelShortName,
  runText,
  subagentDescription,
  tokensText,
  turnDuration,
  workspaceFile,
  type AgentImage,
  type ChangedFile,
  type HistoryBlock,
  type HistoryTurn,
  type LiveBlock,
  type LiveTurn,
  type SessionBackground,
  type SubagentCard,
  type ToolCall,
  type ToolCallStatus,
  type TurnGroup,
  waitingText,
} from "@zwiim/sdk";
import { opensInPreview } from "@zwiim/preview";
import { AgentImageChip } from "@/attachments/AgentImageChip";
import { WorkingMark } from "@/components/WorkingMark";
import { SentAttachments } from "@/attachments/AttachmentChip";
import { FileLinksContext, Markdown } from "@/components/Markdown";
import { basename, parseUnifiedDiff, type FileDiff } from "@/lib/diff";
import { clipToolText, historyTool, liveTool, type ToolSummary } from "@/lib/tools";
import { cn } from "@/lib/utils";

/** How an item that just arrived comes in (DESIGN.md, Motion: medium, ease-out). */
const ENTER = "animate-rise motion-reduce:animate-none";

export function UserBubble({ children, className }: { children: React.ReactNode; className?: string }) {
  return (
    <div
      className={cn(
        "bg-card text-body max-w-[85%] self-end rounded-2xl rounded-br-sm px-3.5 py-2.5 whitespace-pre-wrap",
        className,
      )}
    >
      {children}
    </div>
  );
}

const STATUS_LABEL: Record<string, string> = {
  pending: "pending",
  in_progress: "running",
  completed: "done",
  failed: "failed",
  interrupted: "interrupted",
};

const VERB_ICONS: Record<string, typeof Terminal> = {
  Run: Terminal,
  Read: FileText,
  Edit: FilePen,
  Write: FilePlus2,
  Delete: Trash2,
  Move: ArrowRightLeft,
  Search: Search,
  Fetch: Globe,
  Think: Brain,
  Agent: Bot,
  Todos: ListTodo,
  "Chat compacted": ListCollapse,
  List: FolderOpen,
  View: Image,
  Input: Keyboard,
  Script: FileCode2,
  Ask: MessageCircleQuestion,
};

/**
 * What is open in a session's history (a card, a thought, a tool call), kept
 * above the transcript's rows so a row scrolled out of view and back opens as
 * it was left. Keyed by turn id and place within the turn (a new turn's by
 * the message it answers, until the history has it); gone on reload.
 */
export type KeptState = Map<string, unknown>;
export const KeptStateContext = createContext<KeptState | null>(null);

/** Moves what is kept under the key prefix `from` to `to`. */
export function moveKept(kept: KeptState, from: string, to: string) {
  for (const [key, value] of [...kept]) {
    if (!key.startsWith(from)) continue;
    kept.delete(key);
    kept.set(to + key.slice(from.length), value);
  }
}

/**
 * `useState`, kept under `key` in the session's map when there is one: what
 * a history row holds outlives the row. Without a key, plain state.
 */
function useKeptState<T>(key: string | undefined, initial: T): [T, (next: (prev: T) => T) => void] {
  const kept = useContext(KeptStateContext);
  const [value, setValue] = useState<T>(() =>
    key !== undefined && kept?.has(key) ? (kept.get(key) as T) : initial,
  );
  const set = useCallback(
    (next: (prev: T) => T) =>
      setValue((prev) => {
        const v = next(prev);
        if (key !== undefined) kept?.set(key, v);
        return v;
      }),
    [key, kept],
  );
  return [value, set];
}

/** A row that opens to show more, easing its height open and shut. */
function Disclosure({
  summary,
  children,
  label,
  stateKey,
}: {
  /** The row; given whether it is open when it shows something else then. */
  summary: React.ReactNode | ((open: boolean) => React.ReactNode);
  children?: React.ReactNode;
  label?: string;
  /** Where its open state is kept, for a history row. */
  stateKey?: string;
}) {
  const [open, setOpen] = useKeptState(stateKey, false);
  const id = useId();
  const expandable = Boolean(children);
  const content = typeof summary === "function" ? summary(open) : summary;
  const rowClass = "-mx-2 flex w-[calc(100%+1rem)] min-w-0 items-center gap-2 rounded-md px-2 py-1 text-left";
  // Nothing to open: a plain row, so what is inside it can be its own control.
  if (!expandable) return <div className={rowClass}>{content}</div>;
  return (
    <>
      <button
        type="button"
        className={cn("group/row hover:bg-muted cursor-pointer transition-colors", rowClass)}
        aria-expanded={open}
        aria-controls={id}
        aria-label={label}
        onClick={() => setOpen((o) => !o)}
      >
        {content}
        <ChevronRight
          className={cn(
            "text-muted-foreground ml-auto size-3 shrink-0 opacity-0 transition-[rotate,opacity] duration-180 ease-in-out group-hover/row:opacity-100",
            open && "rotate-90 opacity-100",
          )}
        />
      </button>
      <div
        id={id}
        className={cn(
          // What opens is bigger than a row: the medium duration (DESIGN.md, Motion).
          "grid transition-[grid-template-rows,opacity] duration-280 ease-in-out motion-reduce:transition-none",
          open ? "grid-rows-[1fr] opacity-100" : "grid-rows-[0fr] opacity-0",
        )}
        inert={!open}
      >
        <div className="min-h-0 overflow-hidden">{children}</div>
      </div>
    </>
  );
}

function Output({ text, error }: { text: string; error?: boolean }) {
  return (
    <pre
      className={cn(
        "max-h-72 overflow-auto rounded-lg p-3 font-mono text-xs leading-5 whitespace-pre-wrap",
        error ? "text-destructive bg-destructive/5" : "bg-code",
      )}
    >
      {clipToolText(text)}
    </pre>
  );
}

/** A command and what it printed, as a terminal shows them. */
function CommandCard({ command, output, error }: { command: string; output: string | null; error?: boolean }) {
  return (
    <div className="border-border bg-code overflow-hidden rounded-lg border font-mono text-xs leading-5">
      <pre className="px-3 py-2 break-all whitespace-pre-wrap">
        <span className="text-muted-foreground select-none">$ </span>
        {command}
      </pre>
      {output && (
        <pre
          className={cn(
            "border-border max-h-72 overflow-auto border-t px-3 py-2 break-all whitespace-pre-wrap",
            error ? "text-destructive" : "text-muted-foreground",
          )}
        >
          {clipToolText(output)}
        </pre>
      )}
    </div>
  );
}

function Counts({
  diffs,
  testId = "diff-counts",
}: {
  diffs: readonly { added: number; removed: number }[];
  testId?: string;
}) {
  const added = diffs.reduce((n, d) => n + d.added, 0);
  const removed = diffs.reduce((n, d) => n + d.removed, 0);
  return (
    <span className="flex shrink-0 gap-1 font-mono text-xs" data-testid={testId}>
      <span className="text-done">+{added}</span>
      <span className="text-destructive">−{removed}</span>
    </span>
  );
}

/**
 * One file's change: its path, then its lines with their numbers when known.
 * With `onOpen`, the path opens the whole file.
 */
function DiffCard({ diff, onOpen }: { diff: FileDiff; onOpen?: () => void }) {
  const numbered = diff.lines.some((l) => l.old !== undefined || l.new !== undefined);
  const gutter = "text-faint w-9 shrink-0 pr-2 text-right select-none";
  const header = "bg-card border-border sticky top-0 z-10 flex w-full items-center gap-2 border-b px-3 py-2 text-left";
  const path = diff.path && (
    <>
      <FileText className="text-muted-foreground size-3.5 shrink-0" />
      <span className="min-w-0 truncate">{diff.path}</span>
      <span className="ml-auto">
        <Counts diffs={[diff]} />
      </span>
    </>
  );
  return (
    <div className="font-mono text-xs" data-testid="diff">
      {path &&
        (onOpen ? (
          <button
            type="button"
            className={cn(header, "hover:bg-muted cursor-pointer transition-colors")}
            title={`Open ${diff.path}`}
            onClick={onOpen}
          >
            {path}
          </button>
        ) : (
          <div className={header} title={diff.path ?? undefined}>
            {path}
          </div>
        ))}
      <div className="bg-code py-1 leading-5">
        {diff.lines.map((l, i) =>
          l.kind === "hunk" ? (
            <div key={i} className="text-muted-foreground bg-muted my-1 truncate px-3 select-none">
              ⋯ {l.text}
            </div>
          ) : (
            <div
              key={i}
              className={cn("flex", l.kind === "add" && "bg-add", l.kind === "del" && "bg-del")}
              data-kind={l.kind}
            >
              {numbered && (
                <>
                  <span className={gutter}>{l.old ?? ""}</span>
                  <span className={gutter}>{l.new ?? ""}</span>
                </>
              )}
              <span
                className={cn(
                  "w-5 shrink-0 text-center select-none",
                  l.kind === "add" && "text-done",
                  l.kind === "del" && "text-destructive",
                )}
              >
                {l.kind === "add" ? "+" : l.kind === "del" ? "-" : ""}
              </span>
              <span className="min-w-0 flex-1 pr-3 break-all whitespace-pre-wrap">{l.text || " "}</span>
            </div>
          ),
        )}
      </div>
    </div>
  );
}

/** How long a pointer rests before the card shows, and leaves before it hides. */
const HOVER_OPEN_MS = 150;
const HOVER_CLOSE_MS = 120;
const CARD_WIDTH = 640;
const CARD_MAX_HEIGHT = 440;
const EDGE = 12;

/**
 * A card over the transcript while the pointer rests on its row (or the row
 * has focus, or was clicked). It sits above or below the row, where there is
 * more room, and scrolls on its own; the pointer can move into it.
 */
function HoverCard({
  card,
  children,
  className,
  title,
  onActivate,
}: {
  card: React.ReactNode;
  children: React.ReactNode;
  className?: string;
  title?: string;
  /** What a click does instead of pinning the card open. */
  onActivate?: () => void;
}) {
  const anchor = useRef<HTMLButtonElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const [rect, setRect] = useState<DOMRect | null>(null);
  const id = useId();

  const later = (f: () => void, ms: number) => {
    clearTimeout(timer.current);
    timer.current = setTimeout(f, ms);
  };
  const show = () => {
    clearTimeout(timer.current);
    if (anchor.current) setRect(anchor.current.getBoundingClientRect());
  };
  const hide = () => {
    clearTimeout(timer.current);
    setRect(null);
  };
  // Over the file or the card: a blur from clicking inside it does not close it.
  const pointer = useRef(false);
  const enter = () => {
    pointer.current = true;
    later(show, HOVER_OPEN_MS);
  };
  const leave = () => {
    pointer.current = false;
    later(hide, HOVER_CLOSE_MS);
  };

  useEffect(() => () => clearTimeout(timer.current), []);
  // The card is placed once; when the transcript moves it goes.
  useEffect(() => {
    if (!rect) return;
    const onScroll = (e: Event) => {
      if (!(e.target instanceof Node && panel.current?.contains(e.target))) hide();
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && hide();
    window.addEventListener("scroll", onScroll, true);
    window.addEventListener("resize", hide);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("scroll", onScroll, true);
      window.removeEventListener("resize", hide);
      window.removeEventListener("keydown", onKey);
    };
  }, [rect]);

  let style: React.CSSProperties | undefined;
  if (rect) {
    const width = Math.min(CARD_WIDTH, window.innerWidth - 2 * EDGE);
    const left = Math.max(EDGE, Math.min(rect.left, window.innerWidth - width - EDGE));
    const below = window.innerHeight - rect.bottom - EDGE;
    const above = rect.top - EDGE;
    style =
      below >= Math.min(CARD_MAX_HEIGHT, 240) || below >= above
        ? {
            left,
            width,
            top: rect.bottom + 4,
            maxHeight: Math.min(CARD_MAX_HEIGHT, below - 4),
          }
        : {
            left,
            width,
            bottom: window.innerHeight - rect.top + 4,
            maxHeight: Math.min(CARD_MAX_HEIGHT, above - 4),
          };
  }

  return (
    <button
      type="button"
      ref={anchor}
      className={className}
      title={title}
      aria-expanded={Boolean(rect)}
      onMouseEnter={enter}
      onMouseLeave={leave}
      // Focus from the keyboard opens it; a click's focus is the click's to decide.
      onFocus={(e) => e.target.matches(":focus-visible") && show()}
      onBlur={() => !pointer.current && hide()}
      onClick={() => {
        if (onActivate) {
          hide();
          onActivate();
        } else if (rect) hide();
        else show();
      }}
      aria-describedby={rect ? id : undefined}
    >
      {children}
      {rect &&
        createPortal(
          <div
            ref={panel}
            id={id}
            role="tooltip"
            className="bg-popover text-card-foreground border-border animate-enter shadow-pop fixed z-50 flex flex-col divide-y overflow-auto rounded-lg border text-xs motion-reduce:animate-none"
            style={style}
            onMouseEnter={() => {
              pointer.current = true;
              clearTimeout(timer.current);
            }}
            onMouseLeave={leave}
            data-testid="hover-card"
          >
            {card}
          </div>,
          document.body,
        )}
    </button>
  );
}

/**
 * One tool call: what it does and on what. It opens to a command and its
 * output, an edit's change to its file, or a tool's input and output.
 */
interface ToolCallProps {
  tool: ToolSummary;
  status: ToolCallStatus | null;
  output?: string | null;
  isError?: boolean;
  images?: AgentImage[];
  className?: string;
  /** Where its open state is kept, for a history row. */
  stateKey?: string;
}

/** The same call: its summary is rebuilt on every render, so it is compared by what it says. */
function sameToolCall(a: ToolCallProps, b: ToolCallProps): boolean {
  const keys = new Set([...Object.keys(a), ...Object.keys(b)] as (keyof ToolCallProps)[]);
  for (const k of keys) if (k !== "tool" && !Object.is(a[k], b[k])) return false;
  const t = a.tool;
  const u = b.tool;
  return (
    t === u ||
    (t.verb === u.verb &&
      t.target === u.target &&
      t.input === u.input &&
      t.path === u.path &&
      (t.changes === u.changes || (!t.changes?.length && !u.changes?.length)))
  );
}

export const ToolCallView = memo(function ToolCallView(props: ToolCallProps) {
  const { verb, target } = props.tool;
  const images = props.images ?? [];
  const verbLabel = verb === "Read" && images.length > 0 ? "Read image" : verb;
  const failed = props.status === "failed" || props.isError;
  const running = props.status === "in_progress" || props.status === "pending";
  // A live command's output lives in a terminal the agent does not share.
  const output = props.output?.trim() === "[Terminal]" ? null : (props.output ?? null);
  const diffs = (!failed && output ? parseUnifiedDiff(output) : null) ?? props.tool.changes ?? null;
  const path = diffs?.[0]?.path ?? props.tool.path ?? null;
  const command = verb === "Run" ? props.tool.input || target || null : null;
  // A one-line input already reads in full on the row.
  const input = props.tool.input?.trim() === target ? null : props.tool.input;
  const Icon = VERB_ICONS[verb] ?? Wrench;
  const label = diffs && diffs.length > 1 ? `${diffs.length} files` : path ? basename(path) : target;
  // A file inside the workspace opens whole; several open from the card.
  const links = useContext(FileLinksContext);
  const opener = (p: string | null) => {
    const file = links && workspaceFile(p, links.workspace);
    return file ? () => (opensInPreview(file) ? links.open(file) : links.openDiff(file)) : undefined;
  };
  const title = diffs && diffs.length > 1 ? diffs.map((d) => d.path).join("\n") : (path ?? target);

  // The row's file is the image the call returned: its chip opens it.
  const [image, ...moreImages] = diffs ? [] : images;
  const rest = diffs ? images : moreImages;

  let details: React.ReactNode = null;
  if (command) details = <CommandCard command={command} output={output} error={failed} />;
  else if (input || output)
    details = (
      <>
        {input && <Output text={input} />}
        {output && <Output text={output} error={failed} />}
      </>
    );

  const chip = "bg-muted text-muted-foreground min-w-0 truncate rounded-md px-1.5 py-0.5 font-mono text-xs";
  const row = (
    <Disclosure
      stateKey={props.stateKey}
      summary={
        <>
          <Icon className="text-muted-foreground size-3.5 shrink-0" />
          {/* Running: the verb shimmers; no spinner. */}
          <span className={cn("text-foreground/90 shrink-0", running && "animate-shimmer motion-reduce:animate-none")}>
            {verbLabel}
          </span>
          {image ? (
            <AgentImageChip path={image.path} name={label || image.name} compact />
          ) : (
            label &&
            (diffs ? (
              // The file shows its change over the transcript while the pointer rests on it.
              <HoverCard
                className={cn(chip, "hover:bg-accent hover:text-foreground cursor-pointer transition-colors")}
                title={title}
                onActivate={diffs.length === 1 ? opener(diffs[0]!.path) : undefined}
                card={
                  <>
                    {diffs.map((d, i) => (
                      <DiffCard key={i} diff={d} onOpen={opener(d.path)} />
                    ))}
                    {failed && output && <Output text={output} error />}
                  </>
                }
              >
                <code>{label}</code>
              </HoverCard>
            ) : (
              <code className={cn(chip, running && "animate-fade-in motion-reduce:animate-none")} title={title}>
                {label}
              </code>
            ))
          )}
          {diffs && <Counts diffs={diffs} />}
          {failed && (
            <span className="text-destructive flex shrink-0 items-center gap-0.5 text-xs">
              <X className="size-3" /> failed
            </span>
          )}
          {props.status && !failed && <span className="sr-only">{STATUS_LABEL[props.status] ?? props.status}</span>}
        </>
      }
    >
      {/* The image is the output; the row stays plain so its chip is a control of its own. */}
      {!diffs && !image && details && <div className="mt-1 mb-2 ml-5 flex flex-col gap-2 text-xs">{details}</div>}
    </Disclosure>
  );
  return (
    <div className={cn("text-ui relative", props.className)} data-testid="tool-call" data-status={props.status ?? ""}>
      {row}
      {rest.length > 0 && (
        <div className="mt-1 ml-5 flex flex-wrap gap-1.5">
          {rest.map((img, j) => (
            <AgentImageChip key={j} path={img.path} name={img.name} />
          ))}
        </div>
      )}
    </div>
  );
}, sameToolCall);

/** One row of a reply; a tool call's or sub-agent's says what its run's summary counts. */
interface ReplyRow {
  node: React.ReactNode;
  tool?: { failed: boolean; running: boolean; subagent?: boolean };
}

/** Consecutive tool calls and sub-agents fold into one row from this many, as on the phone. */
const FOLD_AT = 3;

/** A sub-agent card's row: it folds into a run as a tool call does. */
const cardRow = (node: React.ReactNode, card: SubagentCard<unknown>): ReplyRow => ({
  node,
  tool: {
    failed: card.status === "failed",
    running: card.status === "in_progress" || card.status === "pending",
    subagent: true,
  },
});

/**
 * A reply's rows, each run of `FOLD_AT` or more tool calls and sub-agents
 * folded into one row. A run's open state is kept under `scope` and its
 * first row's place.
 */
function foldRuns(rows: readonly (ReplyRow | null)[], scope?: string, className?: string): React.ReactNode[] {
  const out: React.ReactNode[] = [];
  let run: { row: ReplyRow; at: number }[] = [];
  const flush = () => {
    if (run.length >= FOLD_AT) {
      const key = `run:${run[0]!.at}`;
      out.push(
        <ToolRun key={key} rows={run.map((r) => r.row)} stateKey={scope && `${scope}/${key}`} className={className} />,
      );
    } else out.push(...run.map((r) => r.row.node));
    run = [];
  };
  rows.forEach((row, at) => {
    if (!row) return;
    if (row.tool) run.push({ row, at });
    else {
      flush();
      out.push(row.node);
    }
  });
  flush();
  return out;
}

/** A run of tool calls and sub-agents: how many of each and how many failed, open to list them. */
function ToolRun({ rows, stateKey, className }: { rows: readonly ReplyRow[]; stateKey?: string; className?: string }) {
  const agents = rows.filter((r) => r.tool?.subagent).length;
  const failed = rows.filter((r) => r.tool?.failed).length;
  const running = rows.some((r) => r.tool?.running);
  return (
    <div className={cn("text-ui", className)} data-testid="tool-run">
      <Disclosure
        stateKey={stateKey}
        summary={
          <>
            <Layers className="text-muted-foreground size-3.5 shrink-0" />
            {/* One of them running: the count shimmers, as a running row's verb does. */}
            <span
              className={cn("text-foreground/90 shrink-0", running && "animate-shimmer motion-reduce:animate-none")}
            >
              {runText(rows.length - agents, agents)}
            </span>
            {failed > 0 && (
              <span className="text-destructive flex shrink-0 items-center gap-0.5 text-xs">
                <X className="size-3" /> {failed} failed
              </span>
            )}
          </>
        }
      >
        <div className="border-border mt-1 mb-2 ml-1.5 flex flex-col gap-2.5 border-l-2 pl-3">
          {rows.map((r) => r.node)}
        </div>
      </Disclosure>
    </div>
  );
}

/** Thinking: its first line on the row, all of it on expand. */
export function Thought({ text, className, stateKey }: { text: string; className?: string; stateKey?: string }) {
  const body = text.trim();
  // Thinking often opens with a bold heading; the row shows its words.
  const preview = body
    .split("\n")[0]!
    .replace(/^#+\s*|\*\*|__/g, "")
    .trim();
  return (
    <div className={cn("text-ui relative", className)} data-testid="thought">
      <Disclosure
        stateKey={stateKey}
        summary={(open) => (
          <>
            <Brain className="text-muted-foreground size-3.5 shrink-0" />
            <span className="text-foreground/90 shrink-0">Thinking</span>
            {preview && !open && (
              <span className="text-muted-foreground min-w-0 truncate text-xs" data-testid="thought-preview">
                {preview}
              </span>
            )}
          </>
        )}
      >
        {body && (
          <div className="text-muted-foreground border-border mt-1 mb-2 ml-1.5 border-l-2 pl-3">
            <Markdown text={body} />
          </div>
        )}
      </Disclosure>
    </div>
  );
}

/**
 * One sub-agent, at its launch's place: "Agent • Sonnet 5", what it was sent
 * to do, how long it ran. It opens to the prompt it was given (behind its own
 * toggle) and what it did, drawn as the transcript draws a reply; its own
 * sub-agents nest as cards. What it did mounts only while it is open.
 */
function SubagentCardView({
  card,
  title,
  images,
  className,
  stateKey,
  children,
}: {
  card: SubagentCard<unknown>;
  /** The launch's title, for a sub-agent that says nothing of what it does. */
  title?: string | null;
  /** Images its launch returned; shown after what it did. */
  images?: readonly AgentImage[];
  className?: string;
  /** Where its open state (and its prompt's) is kept, for a history row. */
  stateKey?: string;
  /** What it did; rendered only while open. */
  children: () => React.ReactNode;
}) {
  const [open, setOpen] = useKeptState(stateKey, false);
  const [promptOpen, setPromptOpen] = useKeptState(stateKey && `${stateKey}/prompt`, false);
  const id = useId();
  const model = modelShortName(card.model);
  const description = subagentDescription(card.description, title);
  const running = card.status === "in_progress" || card.status === "pending";
  const failed = card.status === "failed";
  const interrupted = card.status === "interrupted";
  const tools = card.tool_count === 1 ? "1 tool" : `${card.tool_count} tools`;
  const prompt = card.prompt?.trim();
  return (
    <div
      className={cn("text-ui relative", className)}
      data-testid="subagent-card"
      data-status={card.status ?? ""}
    >
      <button
        type="button"
        className="group/row hover:bg-muted -mx-2 flex w-[calc(100%+1rem)] min-w-0 cursor-pointer items-center gap-2 rounded-md px-2 py-1 text-left transition-colors"
        aria-expanded={open}
        aria-controls={open ? id : undefined}
        onClick={() => setOpen((o) => !o)}
      >
        <Bot className="text-muted-foreground size-3.5 shrink-0" />
        {/* Running: the name shimmers, as a running tool's verb does. */}
        <span
          className={cn("text-foreground/90 shrink-0", running && "animate-shimmer motion-reduce:animate-none")}
          data-testid="subagent-name"
        >
          Agent
          {model && (
            <>
              <span className="text-muted-foreground"> • </span>
              {model}
            </>
          )}
        </span>
        {description && (
          <span
            className="text-muted-foreground min-w-0 truncate font-mono text-xs"
            title={description}
            data-testid="subagent-description"
          >
            {description}
          </span>
        )}
        <span className="text-muted-foreground ml-auto flex shrink-0 items-center gap-1.5 font-mono text-xs tabular-nums">
          {card.tool_count > 0 && <span data-testid="subagent-tools">{tools}</span>}
          {card.tokens !== null && <span data-testid="subagent-tokens">{tokensText(card.tokens)}</span>}
          {running ? (
            <>
              <WorkingMark />
              <SubagentClock card={card} />
            </>
          ) : (
            card.duration_ms !== null && <span data-testid="subagent-duration">{turnDuration(card.duration_ms)}</span>
          )}
        </span>
        {failed && (
          <span className="text-destructive flex shrink-0 items-center gap-0.5 text-xs">
            <X className="size-3" /> failed
          </span>
        )}
        {/* Stopped unfinished: said plainly, muted, since nothing went wrong in it. */}
        {interrupted && (
          <span className="text-muted-foreground shrink-0 text-xs" data-testid="subagent-interrupted">
            interrupted
          </span>
        )}
        {card.status && !failed && !interrupted && (
          <span className="sr-only">{STATUS_LABEL[card.status] ?? card.status}</span>
        )}
        <ChevronRight
          className={cn(
            "text-muted-foreground size-3 shrink-0 opacity-0 transition-[rotate,opacity] duration-180 ease-in-out group-hover/row:opacity-100",
            open && "rotate-90 opacity-100",
          )}
        />
      </button>
      {open && (
        <div
          id={id}
          className="border-border animate-fade-in mt-1 mb-2 ml-1.5 flex flex-col gap-2.5 border-l-2 pl-3 motion-reduce:animate-none"
          data-testid="subagent-body"
        >
          {prompt && (
            <div>
              <button
                type="button"
                className="text-muted-foreground hover:text-foreground flex cursor-pointer items-center gap-1 text-xs transition-colors"
                aria-expanded={promptOpen}
                onClick={() => setPromptOpen((o) => !o)}
              >
                <ChevronRight
                  className={cn("size-3 transition-[rotate] duration-180 ease-in-out", promptOpen && "rotate-90")}
                />
                Prompt
              </button>
              {promptOpen && (
                <p
                  className="text-muted-foreground bg-code mt-1 max-h-72 overflow-auto rounded-lg p-3 font-mono text-xs leading-5 whitespace-pre-wrap"
                  data-testid="subagent-prompt"
                >
                  {clipToolText(prompt)}
                </p>
              )}
            </div>
          )}
          {children()}
          {!!images?.length && (
            <div className="flex flex-wrap gap-1.5" data-testid="subagent-images">
              {images.map((img, j) => (
                <AgentImageChip key={j} path={img.path} name={img.name} />
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

const userText = (blocks: { type: string; text?: string }[]) =>
  blocks
    .map((b) => (b.type === "text" ? (b.text ?? "") : ""))
    .join("")
    .trim();

/** What the user sent: the files it carried, then its text. */
function UserMessage({
  text,
  blocks,
  className,
}: {
  text: string;
  blocks: readonly { type: string }[];
  className?: string;
}) {
  const files = <SentAttachments blocks={blocks} className={className} />;
  if (!text) return files;
  return (
    <>
      {files}
      <UserBubble className={className}>{text}</UserBubble>
    </>
  );
}

export const HistoryTurnView = memo(function HistoryTurnView({
  turn,
  onRevert,
  revertDisabled,
  background,
  ended = true,
  live,
  carry,
  carried,
}: {
  turn: HistoryTurn;
  /** Offered on a message the Host can put the session back to. */
  onRevert?(): void;
  revertDisabled?: boolean;
  /** The session's background work: a sub-agent still at it reads running. */
  background?: SessionBackground | null;
  /** False while the turn may still be running: an unfinished sub-agent is not yet interrupted. */
  ended?: boolean;
  /** The live turn, when a sub-agent this turn launched is still adding to it. */
  live?: LiveTurn;
  /** The live turn that carries on this turn's run, running or just finished. */
  carry?: LiveTurn;
  /** How many of `carry`'s first blocks show here, folding with this turn's last run. */
  carried?: number;
}) {
  if (turn.role === "user") {
    const text = userText(turn.blocks);
    if (!turn.revertible || !onRevert) return <UserMessage text={text} blocks={turn.blocks} />;
    return (
      <div className="group/reset flex flex-col gap-2.5">
        <UserMessage text={text} blocks={turn.blocks} />
        <button
          type="button"
          className="text-muted-foreground enabled:hover:text-foreground enabled:hover:bg-accent ease-std -mt-1 flex items-center gap-1 self-end rounded-md px-1.5 py-0.5 text-xs opacity-0 transition-[opacity,color,background-color] duration-120 group-focus-within/reset:opacity-100 group-hover/reset:opacity-100 focus-visible:opacity-100 disabled:opacity-0 disabled:group-hover/reset:opacity-50"
          aria-label="Reset to here"
          title="Reset the session to before this message"
          disabled={revertDisabled}
          onClick={onRevert}
        >
          <RotateCcw className="size-3" />
          Reset to here
        </button>
      </div>
    );
  }
  return (
    <div className="text-body flex flex-col gap-2.5">
      <HistoryBlocks
        blocks={turn.blocks}
        background={background ?? null}
        scope={turn.turn_id}
        ended={ended}
        live={live}
        carry={carry}
        carried={carried}
      />
    </div>
  );
});

/**
 * Whether a result shows on its own row: one with a call is shown with it,
 * and one without shows only its images or error.
 */
function shownResult(b: Extract<HistoryBlock, { type: "tool_result" }>, called: ReadonlySet<string>): boolean {
  if (b.tool_call_id && called.has(b.tool_call_id)) return false;
  return !!b.images?.length || !!(b.is_error && b.output_preview);
}

/**
 * A reply's blocks as the transcript shows them, or a sub-agent's: its
 * launches (the tool calls that started a sub-agent) as sub-agent cards. A
 * card ends with what its sub-agent did since the transcript was loaded.
 * The first `carried` blocks of `carry`, the live reply, follow, folding
 * with its last run.
 */
function HistoryBlocks({
  blocks,
  background,
  scope,
  ended,
  live,
  carry,
  carried = 0,
}: {
  blocks: readonly HistoryBlock[];
  background: SessionBackground | null;
  /** The turn's id, or the card's place under it: what open state is kept under. */
  scope: string;
  ended: boolean;
  live: LiveTurn | undefined;
  carry?: LiveTurn;
  carried?: number;
}) {
  // A result is shown with its call; one without a call stands alone.
  const results = new Map<string, Extract<HistoryBlock, { type: "tool_result" }>>();
  for (const b of blocks) if (b.type === "tool_result" && b.tool_call_id) results.set(b.tool_call_id, b);
  const called = new Set(blocks.flatMap((b) => (b.type === "tool_call" && b.tool_call_id ? [b.tool_call_id] : [])));
  const cards = new Map(historySubagentCards(blocks, background, ended).map((c) => [c.parent_id, c]));
  // Calls and cards keep their open state by order within the turn, never by
  // id, so one opened live stays open once the history has it.
  let tools = 0;
  let launches = 0;
  const rows = blocks.map((b, i): ReplyRow | null => {
    switch (b.type) {
      case "text":
        // A blank one would split a run of tool calls for nothing.
        return b.text.trim() ? { node: <Markdown key={i} text={b.text} /> } : null;
      case "thought":
        return {
          node: <Thought key={i} text={b.text} stateKey={`${scope}/${i}`} />,
        };
      case "tool_call": {
        const card = b.tool_call_id ? cards.get(b.tool_call_id) : undefined;
        if (card) {
          // Its result's text repeats the sub-agent's last reply; its images do not.
          const images = results.get(card.parent_id)?.images;
          const key = `agent:${launches++}`;
          const stateKey = `${scope}/${key}`;
          return cardRow(
            <SubagentCardView key={key} card={card} title={b.title} images={images} stateKey={stateKey}>
              {() => {
                const late = live ? lateSubagentBlocks(card, live) : [];
                return (
                  <>
                    <HistoryBlocks
                      blocks={card.blocks}
                      background={background}
                      scope={stateKey}
                      ended={ended && card.status !== "in_progress"}
                      live={live}
                    />
                    {live && late.length > 0 && <LiveBlocks turn={live} blocks={late} background={background} />}
                  </>
                );
              }}
            </SubagentCardView>,
            card,
          );
        }
        const r = b.tool_call_id ? results.get(b.tool_call_id) : undefined;
        // Agents' transcripts often omit the status; a result means it ran.
        const status = r?.is_error ? "failed" : (b.status ?? (r ? "completed" : null));
        return {
          node: (
            <ToolCallView
              key={i}
              tool={historyTool(b.title, b.input_preview)}
              status={status}
              output={r?.output_preview}
              isError={r?.is_error}
              images={r?.images}
              stateKey={`${scope}/tool:${tools++}`}
            />
          ),
          tool: {
            failed: status === "failed",
            running: status === "in_progress" || status === "pending",
          },
        };
      }
      case "tool_result":
        if (!shownResult(b, called)) return null;
        return {
          node: (
            <Fragment key={i}>
              {b.is_error && b.output_preview && <Output text={b.output_preview} error />}
              {!!b.images?.length && (
                <div className="flex flex-wrap gap-1.5">
                  {b.images.map((img, j) => (
                    <AgentImageChip key={j} path={img.path} name={img.name} />
                  ))}
                </div>
              )}
            </Fragment>
          ),
        };
      case "agent_image":
        return { node: <AgentImageChip key={i} path={b.path} name={b.name} /> };
      case "image":
        return null; // older Hosts' inline bytes; agent images now arrive as files
      default:
        return null;
    }
  });
  if (carry && carried > 0) {
    const more = carry.message?.content.slice(0, carried) ?? [];
    const keys = { scope, tools, launches };
    const cardView = liveCardView(carry, background, ENTER, keys);
    const moreRows = liveRows(carry, more, liveSubagentCards(carry, more, background), cardView, ENTER, keys);
    // Keyed apart from the history's own rows.
    for (const [i, r] of moreRows.entries())
      rows.push(r && { ...r, node: <Fragment key={`live:${i}`}>{r.node}</Fragment> });
  }
  return <>{foldRuns(rows, scope)}</>;
}

/**
 * The turn in progress (or one just finished). With `animate`, what arrives
 * eases in; a finished turn shows without it, so it does not arrive twice.
 */
export function LiveTurnView({
  turn,
  streaming,
  animate,
  background,
  earlier,
  from,
  keys,
}: {
  turn: LiveTurn;
  streaming?: boolean;
  animate?: boolean;
  /** The session's background work: a sub-agent still at it reads running. */
  background?: SessionBackground | null;
  /** Launches in the history: their sub-agents' live work shows under their cards there. */
  earlier?: ReadonlySet<string>;
  /** How many of its first blocks show in the history, folded with the run the turn there ends with. */
  from?: number;
  /** Where it keeps what is open, as the history will key it. */
  keys?: LiveKeys;
}) {
  const text = turn.userMessage ? userText(turn.userMessage.blocks) : "";
  const enter = animate ? ENTER : undefined;
  if (!turn.userMessage && !turn.message && !streaming) return null;
  return (
    <div className="text-body flex flex-col gap-2.5" data-testid="live-turn">
      {turn.userMessage && <UserMessage text={text} blocks={turn.userMessage.blocks} className={enter} />}
      <LiveBlocks turn={turn} background={background ?? null} enter={enter} earlier={earlier} from={from} keys={keys} />
      {streaming && <Working className={enter} turnId={turn.userMessage?.message_id} />}
    </div>
  );
}

/**
 * The live reply's blocks, or (given `blocks`) a sub-agent's: its launches
 * as sub-agent cards. The reply's also ends with any sub-agent whose launch
 * it does not hold, so nothing a sub-agent did is hidden, unless its launch
 * is in `earlier`; that card folds with a run it follows, as on the phone.
 * Its first `from` blocks show in the history instead.
 */
function LiveBlocks({
  turn,
  blocks,
  background,
  enter,
  earlier,
  from = 0,
  keys,
}: {
  turn: LiveTurn;
  blocks?: readonly LiveBlock[];
  background: SessionBackground | null;
  enter?: string;
  earlier?: ReadonlySet<string>;
  from?: number;
  keys?: LiveKeys;
}) {
  const scope = blocks ?? turn.message?.content ?? [];
  const all = liveSubagentCards(turn, blocks, background, earlier);
  const inScope = new Set(scope.flatMap((b) => (b.type === "tool_call" ? [b.tool_call_id] : [])));
  const orphans = all.filter((c) => !inScope.has(c.parent_id));
  const cardView = liveCardView(turn, background, enter, keys);
  const rows = liveRows(turn, scope, all, cardView, enter, keys).map((r, i) => (i < from ? null : r));
  rows.push(...orphans.map((c) => cardRow(cardView(c), c)));
  return <>{foldRuns(rows, undefined, enter)}</>;
}

/**
 * Where a live reply keeps what is open, as the history keys its turn: the
 * turn's id, and how many calls and sub-agents come before the reply's in it.
 */
export type LiveKeys = { scope: string; tools: number; launches: number };

/** Draws a live turn's sub-agent cards; they keep their open state by launch order within the turn, never by id. */
function liveCardView(turn: LiveTurn, background: SessionBackground | null, enter?: string, keys?: LiveKeys) {
  let launches = keys?.launches ?? 0;
  return (card: SubagentCard<LiveBlock>) => {
    const key = `agent:${launches++}`;
    const stateKey = keys && `${keys.scope}/${key}`;
    return (
      <SubagentCardView
        key={key}
        className={enter}
        card={card}
        title={turn.toolCalls[card.parent_id]?.title}
        images={turn.toolCalls[card.parent_id]?.images}
        stateKey={stateKey}
      >
        {() => (
          <LiveBlocks
            turn={turn}
            blocks={card.blocks}
            background={background}
            keys={stateKey ? { scope: stateKey, tools: 0, launches: 0 } : undefined}
          />
        )}
      </SubagentCardView>
    );
  };
}

/** The rows of a live turn's `scope`, its launches drawn by `cardView`; its calls keep what is open under `keys`. */
function liveRows(
  turn: LiveTurn,
  scope: readonly LiveBlock[],
  all: readonly SubagentCard<LiveBlock>[],
  cardView: (card: SubagentCard<LiveBlock>) => React.ReactNode,
  enter?: string,
  keys?: LiveKeys,
): (ReplyRow | null)[] {
  const cards = new Map(all.map((c) => [c.parent_id, c]));
  let tools = keys?.tools ?? 0;
  return scope.map((b, i): ReplyRow | null => {
    if (b.type === "text") return b.text.trim() ? { node: <Markdown key={i} className={enter} text={b.text} /> } : null;
    if (b.type === "thought") return { node: <Thought key={i} className={enter} text={b.text} /> };
    if (b.type === "agent_image") return { node: <AgentImageChip key={i} path={b.path} name={b.name} /> };
    const card = cards.get(b.tool_call_id);
    if (card) return cardRow(cardView(card), card);
    const tc: ToolCall | undefined = turn.toolCalls[b.tool_call_id];
    const status = tc?.status ?? null;
    const n = tools++;
    return {
      node: (
        <ToolCallView
          key={i}
          className={enter}
          tool={liveTool(tc?.title ?? null, tc?.kind)}
          status={status}
          output={typeof tc?.content === "string" ? tc.content : null}
          images={tc?.images}
          stateKey={keys && `${keys.scope}/tool:${n}`}
        />
      ),
      tool: {
        failed: status === "failed",
        running: status === "in_progress" || status === "pending",
      },
    };
  });
}

/**
 * How many of the live reply's first blocks carry on the run of tool calls
 * and sub-agents `turn` ends with: the history's turn the agent is still
 * running, and those blocks fold with its run there. None when it ends
 * otherwise.
 */
export function carriedBlocks(turn: HistoryTurn, live: LiveTurn): number {
  if (turn.role === "user") return 0;
  const called = new Set(
    turn.blocks.flatMap((b) => (b.type === "tool_call" && b.tool_call_id ? [b.tool_call_id] : [])),
  );
  // Its last block that shows as a row, as HistoryBlocks draws them.
  const last = [...turn.blocks].reverse().find((b) => {
    if (b.type === "text") return !!b.text.trim();
    if (b.type === "tool_result") return shownResult(b, called);
    return b.type === "thought" || b.type === "tool_call" || b.type === "agent_image";
  });
  if (last?.type !== "tool_call") return 0;
  let n = 0;
  for (const [i, b] of (live.message?.content ?? []).entries()) {
    if (b.type === "tool_call") n = i + 1;
    else if (b.type !== "text" || b.text.trim()) break;
  }
  return n;
}

/** Where the live reply that carries on `turn` keeps what is open: after the turn's own calls and sub-agents. */
export function continuedKeys(turn: HistoryTurn): LiveKeys | undefined {
  if (turn.role === "user") return undefined;
  const launches = turn.blocks.filter(isLaunch).length;
  const calls = turn.blocks.filter((b) => b.type === "tool_call").length;
  return { scope: turn.turn_id, tools: calls - launches, launches };
}

/** Where a new turn's live reply keeps what is open until the history has the turn: under the message it answers. */
export function sentKeys(turn: LiveTurn): LiveKeys | undefined {
  return turn.userMessage ? { scope: `live:${turn.userMessage.message_id}`, tools: 0, launches: 0 } : undefined;
}

/** When each live turn was first seen, so its timer survives a remount (switching sessions). */
const turnStarts = new Map<string, number>();

function turnStart(id: string | undefined): number {
  return id ? firstSeen(turnStarts, id) : Date.now();
}

/**
 * How long a running sub-agent has been at it, counting each second: from
 * when the Host says it started, else from when its launch was first seen.
 */
function SubagentClock({ card }: { card: SubagentCard<unknown> }) {
  const since = card.started_at ?? launchFirstSeen(card.parent_id);
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);
  return <span data-testid="subagent-elapsed">{turnDuration(Math.max(0, now - since))}</span>;
}

/** While the agent works: four dots lighting in turn, and how long it has been at it. */
function Working({ className, turnId }: { className?: string; turnId?: string }) {
  const [since] = useState(() => turnStart(turnId));
  const [now, setNow] = useState(since);
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);
  return (
    <span className={cn("text-muted-foreground flex items-center gap-2 text-xs", className)} data-testid="working">
      <WorkingMark />
      Working
      <span className="font-mono tabular-nums">{turnDuration(now - since)}</span>
    </span>
  );
}

/**
 * The agent has nothing left to do but wait for its background work: how
 * many tasks and since when, counting each second. Nothing otherwise.
 */
export function BackgroundWaiting({
  background,
  className,
}: {
  background: SessionBackground | null | undefined;
  className?: string;
}) {
  const parsed =
    background && background.outstanding > 0 && background.waiting_since
      ? Date.parse(background.waiting_since)
      : NaN;
  // A time it cannot read counts as not waiting, as on Mobile.
  const since = Number.isNaN(parsed) ? null : parsed;
  const [now, setNow] = useState(() => Date.now());
  const shown = since !== null;
  useEffect(() => {
    if (!shown) return;
    setNow(Date.now());
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [shown]);
  if (!background || since === null) return null;
  return (
    <p
      className={cn("text-muted-foreground flex items-center gap-2 text-xs", className)}
      role="status"
      data-testid="background-waiting"
    >
      <WorkingMark />
      {waitingText(background.outstanding)}
      {/* Hidden from screen readers: a live region must not announce every second. */}
      <span className="font-mono tabular-nums" aria-hidden="true" data-testid="background-waiting-time">
        {turnDuration(now - since)}
      </span>
    </p>
  );
}

/** Files a turn's row shows at first, and how many more each click reveals. */
const FOOTER_FILES = 2;

/** Under a finished turn: how long it ran, when it ended, and the files it changed. */
export const TurnFooter = memo(function TurnFooter({
  group,
  stateKey,
}: {
  group: TurnGroup;
  /** Where how many files it shows is kept. */
  stateKey?: string;
}) {
  const [shown, setShown] = useKeptState(stateKey, FOOTER_FILES);
  const time = [
    group.durationMs !== null ? turnDuration(group.durationMs) : "",
    group.completedAt ? clockTime(group.completedAt) : "",
  ]
    .filter(Boolean)
    .join(" · ");
  const files = group.files;
  const rest = files.slice(shown);
  if (!time && files.length === 0) return null;
  const more =
    "bg-muted hover:bg-accent hover:text-foreground flex items-center gap-1.5 rounded-md px-1.5 py-0.5 transition-colors";
  return (
    <div
      className="text-muted-foreground -mt-2 flex flex-wrap items-center gap-1.5 font-mono text-xs"
      data-testid="turn-footer"
    >
      {time && (
        <span className="mr-1" data-testid="turn-time">
          {time}
        </span>
      )}
      {files.slice(0, shown).map((f) => (
        <FileChip key={f.path} file={f} />
      ))}
      {rest.length > 0 ? (
        <button
          type="button"
          className={more}
          onClick={() => setShown((n) => n + FOOTER_FILES)}
          title={rest.map((f) => f.path).join("\n")}
        >
          +{rest.length} more
          <Counts diffs={rest} testId="file-counts" />
        </button>
      ) : (
        files.length > FOOTER_FILES && (
          <button type="button" className={more} onClick={() => setShown(() => FOOTER_FILES)}>
            Show less
          </button>
        )
      )}
    </div>
  );
});

/** A file a turn changed; opens it whole when it is inside the workspace. */
function FileChip({ file }: { file: ChangedFile }) {
  const links = useContext(FileLinksContext);
  const path = links && workspaceFile(file.path, links.workspace);
  const chip = "bg-muted flex max-w-56 min-w-0 items-center gap-1.5 rounded-md px-1.5 py-0.5";
  const content = (
    <>
      <FileText className="size-3 shrink-0" />
      <span className="text-foreground/90 min-w-0 truncate">{basename(file.path)}</span>
      <Counts diffs={[file]} testId="file-counts" />
    </>
  );
  return path ? (
    <button
      type="button"
      className={cn(chip, "hover:bg-accent hover:text-foreground transition-colors")}
      title={file.path}
      onClick={() => (opensInPreview(path) ? links.open(path) : links.openDiff(path))}
      data-testid="turn-file"
    >
      {content}
    </button>
  ) : (
    <span className={chip} title={file.path} data-testid="turn-file">
      {content}
    </span>
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
