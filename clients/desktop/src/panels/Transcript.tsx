import { Fragment, useContext, useEffect, useId, useRef, useState } from "react";
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
  ListCollapse,
  ListTodo,
  MessageCircleQuestion,
  Search,
  Terminal,
  Trash2,
  Wrench,
  X,
} from "lucide-react";
import {
  clockTime,
  turnDuration,
  workspaceFile,
  type AgentImage,
  type ChangedFile,
  type HistoryBlock,
  type HistoryTurn,
  type LiveTurn,
  type ToolCall,
  type ToolCallStatus,
  type TurnGroup,
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

/** A row that opens to show more, easing its height open and shut. */
function Disclosure({
  summary,
  children,
  label,
}: {
  /** The row; given whether it is open when it shows something else then. */
  summary: React.ReactNode | ((open: boolean) => React.ReactNode);
  children?: React.ReactNode;
  label?: string;
}) {
  const [open, setOpen] = useState(false);
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
export function ToolCallView(props: {
  tool: ToolSummary;
  status: ToolCallStatus | null;
  output?: string | null;
  isError?: boolean;
  images?: AgentImage[];
  className?: string;
}) {
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
}

/** Thinking: its first line on the row, all of it on expand. */
export function Thought({ text, className }: { text: string; className?: string }) {
  const body = text.trim();
  // Thinking often opens with a bold heading; the row shows its words.
  const preview = body
    .split("\n")[0]!
    .replace(/^#+\s*|\*\*|__/g, "")
    .trim();
  return (
    <div className={cn("text-ui relative", className)} data-testid="thought">
      <Disclosure
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

export function HistoryTurnView({ turn }: { turn: HistoryTurn }) {
  if (turn.role === "user") {
    const text = userText(turn.blocks);
    return <UserMessage text={text} blocks={turn.blocks} />;
  }
  // A result is shown with its call; one without a call stands alone.
  const results = new Map<string, Extract<HistoryBlock, { type: "tool_result" }>>();
  for (const b of turn.blocks) if (b.type === "tool_result" && b.tool_call_id) results.set(b.tool_call_id, b);
  const called = new Set(
    turn.blocks.flatMap((b) => (b.type === "tool_call" && b.tool_call_id ? [b.tool_call_id] : [])),
  );
  return (
    <div className="text-body flex flex-col gap-2.5">
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
                tool={historyTool(b.title, b.input_preview)}
                // Agents' transcripts often omit the status; a result means it ran.
                status={r?.is_error ? "failed" : (b.status ?? (r ? "completed" : null))}
                output={r?.output_preview}
                isError={r?.is_error}
                images={r?.images}
              />
            );
          }
          case "tool_result":
            if (b.tool_call_id && called.has(b.tool_call_id)) return null;
            if (!b.images?.length && !(b.is_error && b.output_preview)) return null;
            return (
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
            );
          case "agent_image":
            return <AgentImageChip key={i} path={b.path} name={b.name} />;
          case "image":
            return null; // older Hosts' inline bytes; agent images now arrive as files
          default:
            return null;
        }
      })}
    </div>
  );
}

/**
 * The turn in progress (or one just finished). With `animate`, what arrives
 * eases in; a finished turn shows without it, so it does not arrive twice.
 */
export function LiveTurnView({ turn, streaming, animate }: { turn: LiveTurn; streaming?: boolean; animate?: boolean }) {
  const text = turn.userMessage ? userText(turn.userMessage.blocks) : "";
  const toolCall = (id: string): ToolCall | undefined => turn.toolCalls[id];
  const enter = animate ? ENTER : undefined;
  if (!turn.userMessage && !turn.message && !streaming) return null;
  return (
    <div className="text-body flex flex-col gap-2.5" data-testid="live-turn">
      {turn.userMessage && <UserMessage text={text} blocks={turn.userMessage.blocks} className={enter} />}
      {turn.message?.content.map((b, i) => {
        if (b.type === "text") return <Markdown key={i} className={enter} text={b.text} />;
        if (b.type === "thought") return <Thought key={i} className={enter} text={b.text} />;
        if (b.type === "agent_image") return <AgentImageChip key={i} path={b.path} name={b.name} />;
        const tc = toolCall(b.tool_call_id);
        return (
          <ToolCallView
            key={i}
            className={enter}
            tool={liveTool(tc?.title ?? null, tc?.kind)}
            status={tc?.status ?? null}
            output={typeof tc?.content === "string" ? tc.content : null}
            images={tc?.images}
          />
        );
      })}
      {streaming && <Working className={enter} turnId={turn.userMessage?.message_id} />}
    </div>
  );
}

/** When each live turn was first seen, so its timer survives a remount (switching sessions). */
const turnStarts = new Map<string, number>();

function turnStart(id: string | undefined): number {
  if (!id) return Date.now();
  let at = turnStarts.get(id);
  if (at === undefined) {
    at = Date.now();
    turnStarts.set(id, at);
    if (turnStarts.size > 64) turnStarts.delete(turnStarts.keys().next().value!);
  }
  return at;
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

/** Files a turn's row shows at first, and how many more each click reveals. */
const FOOTER_FILES = 2;

/** Under a finished turn: how long it ran, when it ended, and the files it changed. */
export function TurnFooter({ group }: { group: TurnGroup }) {
  const [shown, setShown] = useState(FOOTER_FILES);
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
          <button type="button" className={more} onClick={() => setShown(FOOTER_FILES)}>
            Show less
          </button>
        )
      )}
    </div>
  );
}

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
