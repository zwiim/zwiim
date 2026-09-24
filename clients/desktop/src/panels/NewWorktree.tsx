import { useEffect, useRef, useState } from "react";
import {
  Check,
  ChevronDown,
  CornerDownLeft,
  FolderGit2,
  GitBranch,
  ListChecks,
  Loader2,
  Map,
  Paperclip,
  Plus,
  X,
} from "lucide-react";
import type { PromptBlock, WorktreeBases } from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import { errorMessage, useController, useHost, useOnline } from "@/lib/host-context";
import { cn } from "@/lib/utils";
import { ControlSelect } from "./OptionsBar";
import { NewSessionControls, NoAgents, startParams, useNewSessionAgent } from "./SessionPanel";
import { useSlashMenu } from "./SlashCommands";

/** Images only: what the agents that take attachments can read. */
const ATTACHMENT_MAX_BYTES = 5 * 1024 * 1024;

interface Attachment {
  id: string;
  name: string;
  mime_type: string;
  /** Base64, without the data-URL prefix. */
  data: string;
}

function readImage(file: File): Promise<Attachment> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const url = String(reader.result);
      resolve({
        id: crypto.randomUUID(),
        name: file.name || "Pasted image",
        mime_type: file.type,
        data: url.slice(url.indexOf(",") + 1),
      });
    };
    reader.onerror = () => reject(reader.error ?? new Error(`Could not read ${file.name}`));
    reader.readAsDataURL(file);
  });
}

/**
 * A new worktree of a repository, and the task its first session starts on.
 * Two host operations: create the worktree, then start a session in it. If the
 * start fails the worktree stays; trying again only starts the session.
 */
export function NewWorktree({
  rootId,
  onClose,
  onCreated,
  onStarted,
}: {
  /** The repository preselected. */
  rootId: string;
  onClose(): void;
  onCreated(workspaceId: string): void;
  onStarted(workspaceId: string, sessionId: string): void;
}) {
  const controller = useController();
  const online = useOnline();
  const workspaces = useHost((s) => s.workspaces);
  const roots = (workspaces ?? []).filter((w) => w.kind === "root" && !w.removed);
  const [repo, setRepo] = useState(rootId);
  const [bases, setBases] = useState<WorktreeBases | null>(null);
  const [base, setBase] = useState<string | null>(null);
  const [text, setText] = useState("");
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  const [menu, setMenu] = useState(false);
  const [created, setCreated] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const picker = useNewSessionAgent();
  const { agents, agent, advertised, start } = picker;
  const fileInput = useRef<HTMLInputElement>(null);
  const area = useRef<HTMLTextAreaElement>(null);
  const slash = useSlashMenu({ text, setText, area, commands: picker.commands, below: true });
  const prompt = text.trim();
  const task = prompt !== "" || attachments.length > 0;
  const canPlan = advertised?.mode?.available.includes("plan") ?? false;
  const planning = start?.shown.mode?.current === "plan";

  useEffect(() => {
    let live = true;
    setBases(null);
    setBase(null);
    controller.client
      .request("git.branches", { workspace_id: repo })
      .then((b) => {
        if (!live) return;
        setBases(b);
        setBase(b.default);
      })
      .catch(() => {
        // Without the list the worktree starts from the root's HEAD.
        if (live) setBases({ local: [], remote: [], default: null });
      });
    return () => {
      live = false;
    };
  }, [controller, repo]);

  useEffect(() => {
    const close = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !pending) onClose();
    };
    document.addEventListener("keydown", close);
    return () => document.removeEventListener("keydown", close);
  }, [onClose, pending]);

  async function attach(files: Iterable<File>) {
    const images = [...files].filter((f) => f.type.startsWith("image/"));
    const fits = images.filter((f) => f.size <= ATTACHMENT_MAX_BYTES);
    if (images.length === 0) setError("Only images can be attached.");
    else if (fits.length < images.length) setError("Images larger than 5 MB were not attached.");
    else setError(null);
    try {
      const read = await Promise.all(fits.map(readImage));
      setAttachments((a) => [...a, ...read]);
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  function togglePlan() {
    if (!canPlan || !advertised?.mode) return;
    picker.pick({ mode: planning ? advertised.mode.current : "plan" });
  }

  async function create() {
    if (pending || !online || bases === null || (task && agent === null)) return;
    setPending(true);
    setError(null);
    let target = created;
    try {
      if (!target) {
        target = (await controller.createWorktree(repo, base ? { base } : {})).workspace_id;
        setCreated(target);
        onCreated(target);
      }
      const blocks: PromptBlock[] = [
        ...(prompt ? [{ type: "text" as const, text: prompt }] : []),
        ...attachments.map((a) => ({ type: "image" as const, data: a.data, mime_type: a.mime_type })),
      ];
      const params = task ? startParams(picker, target, blocks, crypto.randomUUID()) : null;
      if (params) onStarted(target, await controller.startSession(params));
      onClose();
    } catch (e) {
      setError(
        target ? `The worktree was created, but the session did not start: ${errorMessage(e)}` : errorMessage(e),
      );
    } finally {
      setPending(false);
    }
  }

  return (
    <div
      className="fixed inset-0 z-50 grid place-items-center bg-black/40 p-6"
      onPointerDown={(e) => {
        if (e.target === e.currentTarget && !pending) onClose();
      }}
    >
      <div
        className="bg-card text-card-foreground ring-border flex w-full max-w-2xl flex-col rounded-2xl shadow-2xl ring-1"
        role="dialog"
        aria-modal="true"
        aria-label="New worktree"
      >
        <div className="flex items-center gap-1 border-b px-3 py-2">
          <FolderGit2 className="text-muted-foreground ml-1 size-4 shrink-0" />
          <ControlSelect
            className="text-foreground text-sm font-medium"
            label="Repository"
            value={repo}
            disabled={created !== null || pending}
            onChange={setRepo}
            options={roots.map((w) => ({ value: w.workspace_id, label: w.alias ?? w.name }))}
          />
          <span className="text-muted-foreground text-sm">from</span>
          <BranchPicker bases={bases} value={base} disabled={created !== null || pending} onChange={setBase} />
          <Button className="ml-auto" size="icon-sm" variant="ghost" title="Close" disabled={pending} onClick={onClose}>
            <X />
          </Button>
        </div>
        <div className="relative">
          <textarea
            ref={area}
            className="placeholder:text-muted-foreground block min-h-36 w-full resize-none bg-transparent px-4 pt-3 pb-1 leading-relaxed outline-none"
            placeholder="What do you want to work on?"
            aria-label="What do you want to work on?"
            autoFocus
            value={text}
            disabled={pending}
            onChange={(e) => {
              setText(e.target.value);
              slash.track(e);
            }}
            onSelect={slash.track}
            onPaste={(e) => {
              const files = [...e.clipboardData.files];
              if (files.length === 0) return;
              e.preventDefault();
              void attach(files);
            }}
            onKeyDown={(e) => {
              if (slash.onKeyDown(e)) return;
              if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
                e.preventDefault();
                void create();
              } else if (e.key === "Tab" && e.shiftKey && canPlan) {
                e.preventDefault();
                togglePlan();
              } else if (e.key.toLowerCase() === "u" && (e.metaKey || e.ctrlKey)) {
                e.preventDefault();
                fileInput.current?.click();
              }
            }}
          />
          {slash.menu}
        </div>
        <input
          ref={fileInput}
          type="file"
          accept="image/*"
          multiple
          hidden
          data-testid="attachment-input"
          onChange={(e) => {
            if (e.target.files) void attach(e.target.files);
            e.target.value = "";
          }}
        />
        {(attachments.length > 0 || planning) && (
          <div className="flex flex-wrap items-center gap-1.5 px-4 pb-2" data-testid="attachments">
            {planning && (
              <span className="flex h-7 items-center gap-1.5 rounded-md bg-sky-500/10 px-2 text-xs text-sky-700 dark:text-sky-300">
                <ListChecks className="size-3.5" /> Plan mode
                <button className="hover:text-foreground" aria-label="Leave plan mode" onClick={togglePlan}>
                  <X className="size-3" />
                </button>
              </span>
            )}
            {attachments.map((a) => (
              <span key={a.id} className="bg-muted flex h-7 max-w-48 items-center gap-1.5 rounded-md pr-2 text-xs">
                <img
                  className="size-7 shrink-0 rounded-l-md object-cover"
                  src={`data:${a.mime_type};base64,${a.data}`}
                  alt=""
                />
                <span className="truncate">{a.name}</span>
                <button
                  className="text-muted-foreground hover:text-foreground shrink-0"
                  aria-label={`Remove ${a.name}`}
                  onClick={() => setAttachments((all) => all.filter((x) => x.id !== a.id))}
                >
                  <X className="size-3" />
                </button>
              </span>
            ))}
          </div>
        )}
        {agents !== null && agent === null && (
          <div className="px-4 pb-2">
            <NoAgents />
          </div>
        )}
        {error && (
          <p className="text-destructive px-4 pb-2 text-xs" role="alert">
            {error}
          </p>
        )}
        <div className="flex items-center gap-2 px-2 pb-2">
          <div className="flex min-w-0 flex-1 items-center gap-0.5">
            {/* Plan mode is in the + menu; unattended stays a choice made inside a session. */}
            <NewSessionControls
              picker={{ ...picker, start: start && { ...start, shown: { ...start.shown, mode: null } } }}
              disabled={!online || pending}
            />
          </div>
          <div className="relative shrink-0">
            <Button
              size="icon-sm"
              variant="ghost"
              title="More options"
              aria-haspopup="menu"
              aria-expanded={menu}
              disabled={pending}
              onClick={() => setMenu((m) => !m)}
            >
              <Plus />
            </Button>
            {menu && (
              <PlusMenu
                canPlan={canPlan}
                planning={planning}
                onClose={() => {
                  setMenu(false);
                  area.current?.focus();
                }}
                onAttach={() => fileInput.current?.click()}
                onPlan={togglePlan}
              />
            )}
          </div>
          <Button
            className="bg-foreground text-background hover:bg-foreground/85 shrink-0"
            size="sm"
            disabled={pending || !online || bases === null || (task && agent === null)}
            onClick={() => void create()}
          >
            {pending && <Loader2 className="animate-spin" />}
            {created ? "Start session" : "Create"}
            {!pending && <CornerDownLeft className="opacity-60" />}
          </Button>
        </div>
      </div>
    </div>
  );
}

/** Attach images, or start in plan mode. */
function PlusMenu({
  canPlan,
  planning,
  onClose,
  onAttach,
  onPlan,
}: {
  canPlan: boolean;
  planning: boolean;
  onClose(): void;
  onAttach(): void;
  onPlan(): void;
}) {
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => {
    root.current?.querySelector<HTMLButtonElement>("button")?.focus();
    const close = (e: PointerEvent) => {
      // The + button toggles the menu itself.
      if (!root.current?.parentElement?.contains(e.target as Node)) onClose();
    };
    document.addEventListener("pointerdown", close);
    return () => document.removeEventListener("pointerdown", close);
  }, [onClose]);
  const item =
    "hover:bg-muted focus-visible:bg-muted flex w-full items-center gap-2.5 rounded-lg px-2.5 py-2 text-left text-sm outline-none disabled:opacity-50";
  return (
    <div
      ref={root}
      className="bg-card text-card-foreground absolute right-0 bottom-full z-20 mb-2 w-60 rounded-xl border p-1 shadow-xl"
      role="menu"
      aria-label="More options"
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          e.stopPropagation();
          onClose();
        }
      }}
    >
      <button
        className={item}
        role="menuitem"
        onClick={() => {
          onClose();
          onAttach();
        }}
      >
        <Paperclip className="text-muted-foreground size-4" />
        <span className="flex-1">Add attachment</span>
        <kbd className="text-muted-foreground font-sans text-xs">⌘U</kbd>
      </button>
      <button
        className={item}
        role="menuitemcheckbox"
        aria-checked={planning}
        disabled={!canPlan}
        title={canPlan ? undefined : "This agent has no plan mode"}
        onClick={() => {
          onPlan();
          onClose();
        }}
      >
        <Map className="text-muted-foreground size-4" />
        <span className="flex-1">Plan mode</span>
        {planning ? <Check className="size-4" /> : <kbd className="text-muted-foreground font-sans text-xs">⇧Tab</kbd>}
      </button>
    </div>
  );
}

/** The branch a new worktree starts from: a searchable list of the host's branches. */
function BranchPicker({
  bases,
  value,
  disabled,
  onChange,
}: {
  bases: WorktreeBases | null;
  value: string | null;
  disabled: boolean;
  onChange(branch: string): void;
}) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [at, setAt] = useState(0);
  const root = useRef<HTMLDivElement>(null);
  const all = bases ? [...bases.remote, ...bases.local] : [];
  const q = query.trim().toLowerCase();
  const shown = q ? all.filter((b) => b.toLowerCase().includes(q)) : all;

  useEffect(() => {
    if (!open) return;
    setQuery("");
    setAt(Math.max(0, all.indexOf(value ?? "")));
    const close = (e: PointerEvent) => {
      if (!root.current?.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("pointerdown", close);
    return () => document.removeEventListener("pointerdown", close);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  useEffect(() => {
    root.current?.querySelector(`[data-index="${at}"]`)?.scrollIntoView?.({ block: "nearest" });
  }, [at]);

  function pick(branch: string | undefined) {
    if (!branch) return;
    onChange(branch);
    setOpen(false);
  }

  return (
    <div ref={root} className="relative min-w-0">
      <button
        type="button"
        className="text-muted-foreground hover:text-foreground hover:bg-muted focus-visible:ring-ring/50 flex h-7 min-w-0 items-center gap-1 rounded-md px-2 text-sm outline-none focus-visible:ring-2 disabled:opacity-60 disabled:hover:bg-transparent"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label={`Base branch: ${value ?? "current HEAD"}`}
        title="The branch the worktree starts from"
        disabled={disabled || bases === null}
        onClick={() => setOpen((o) => !o)}
      >
        <GitBranch className="size-3.5 shrink-0" />
        <span className="truncate font-mono text-xs">{bases === null ? "…" : (value ?? "HEAD")}</span>
        <ChevronDown className="size-3 shrink-0" />
      </button>
      {open && (
        <div
          className="bg-card text-card-foreground absolute top-full left-0 z-20 mt-1 flex w-72 flex-col rounded-xl border shadow-xl"
          onKeyDown={(e) => {
            if (e.key === "Escape") {
              e.stopPropagation();
              setOpen(false);
            } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
              e.preventDefault();
              const n = shown.length;
              if (n) setAt((i) => (i + (e.key === "ArrowDown" ? 1 : n - 1)) % n);
            } else if (e.key === "Enter") {
              e.preventDefault();
              pick(shown[at]);
            }
          }}
        >
          <input
            className="placeholder:text-muted-foreground border-b bg-transparent px-3 py-2 text-sm outline-none"
            placeholder="Select base branch…"
            aria-label="Search branches"
            autoFocus
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setAt(0);
            }}
          />
          <ul className="max-h-64 overflow-y-auto p-1" role="listbox" aria-label="Branches">
            {shown.length === 0 && <li className="text-muted-foreground px-2.5 py-2 text-xs">No matching branch</li>}
            {shown.map((b, i) => (
              <li
                key={b}
                data-index={i}
                role="option"
                aria-selected={b === value}
                className={cn(
                  "flex cursor-pointer items-center gap-2 rounded-lg px-2.5 py-1.5 font-mono text-xs",
                  i === at && "bg-muted",
                )}
                onPointerEnter={() => setAt(i)}
                onClick={() => pick(b)}
              >
                <Check className={cn("size-3.5 shrink-0", b !== value && "invisible")} />
                <span className="truncate">{b}</span>
                {b === bases?.default && <span className="text-muted-foreground ml-auto font-sans">default</span>}
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}
