import { useEffect, useId, useRef, useState } from "react";
import { ArrowUpRight, Loader2 } from "lucide-react";
import {
  isClientError,
  largeFolderDetails,
  largeFolderText,
  publishFailedDetails,
  type AddOptions,
  type WorkspaceInfo,
  type WorkspaceInspection,
} from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import { openLink } from "@/lib/bridge";
import { errorMessage, useController, useOnline } from "@/lib/host-context";
import { cn } from "@/lib/utils";
import { ControlSelect } from "./OptionsBar";

/** The folder being added: picked by path, or a root already in the list. */
export type PublishSubject = { kind: "path"; path: string } | { kind: "root"; workspace_id: string };

export interface PublishDialogProps {
  subject: PublishSubject;
  inspection: WorkspaceInspection;
  /** Whether the Host is another computer ("the host" in copy) or this one ("this Mac"). */
  remote: boolean;
  onClose(): void;
  /** The folder was added and published; the caller opens its first worktree. */
  onAdded(workspace: WorkspaceInfo): void;
}

/** How long the name must stay unchanged before it is checked on GitHub. */
const CHECK_DELAY_MS = 300;
/** How often, and for how long, the folder is read again after an add with `publish` outlasted the SDK's timeout. */
const WATCH_EVERY_MS = 10_000;
const WATCH_FOR_MS = 10 * 60_000;

/** The Host's repository name rule (`publish.rs` `valid_repo_name`): null when the name keeps it. */
function nameRule(name: string): string | null {
  const keeps = /^[A-Za-z0-9._-]{1,100}$/.test(name) && name !== "." && name !== "..";
  return keeps ? null : "Repository names use letters, digits, '.', '-' and '_' (at most 100).";
}

type Verdict = "available" | "taken" | "failed";

/** Why the Host's GitHub CLI can't publish (`workspace.inspect`'s `github.reason`, or the add's error code). */
const UNAVAILABLE = ["gh_unavailable", "gh_unauthenticated", "github_unreachable"] as const;
export type Unavailable = (typeof UNAVAILABLE)[number];

/**
 * What the dialog shows: the owner and name fields, or one line in their
 * place: GitHub can't be used, HEAD is detached, the folder already has a
 * remote (found by Try again), GitHub created the repository but the push
 * to it failed, or the add outlasted the SDK's timeout: the Host is still
 * working on it (`watching`), or didn't finish in time (`unfinished`).
 */
type Phase =
  | { kind: "form" }
  | { kind: "unavailable"; reason: Unavailable }
  | { kind: "detached" }
  | { kind: "has_origin" }
  | { kind: "failed"; owner: string; name: string }
  | { kind: "watching" }
  | { kind: "unfinished" };

function phaseOf(inspection: WorkspaceInspection): Phase {
  if (inspection.git === "origin") return { kind: "has_origin" };
  if (inspection.git === "no_origin" && inspection.branch === null) return { kind: "detached" };
  if (inspection.github?.available === false) return { kind: "unavailable", reason: inspection.github.reason };
  return { kind: "form" };
}

/** DESIGN.md's `primary` button: the accent, `brand` on Desktop (as Continue after a merge). */
const PRIMARY = "bg-brand text-brand-foreground hover:bg-brand/85";

export const Code = ({ children }: { children: React.ReactNode }) => <code className="font-mono text-xs">{children}</code>;

/** The folder's name, as the Host names a root added from it. */
const folderName = (path: string) => path.replace(/\/+$/, "").split("/").pop() ?? "";

export function unavailableLine(reason: Unavailable, host: string) {
  switch (reason) {
    case "gh_unavailable":
      return <>GitHub CLI isn't installed on {host}.</>;
    case "gh_unauthenticated":
      return (
        <>
          GitHub CLI isn't signed in on {host}. Run <Code>gh auth login</Code>, then try again.
        </>
      );
    case "github_unreachable":
      return <>Zwiim can't reach GitHub.</>;
  }
}

/**
 * Asks before a folder becomes a private GitHub repository (D11): only the
 * click on its button creates one, under the owner and name shown, which the
 * user can change and which are checked on GitHub as they type.
 */
export function PublishDialog({ subject, remote, onClose, onAdded, ...props }: PublishDialogProps) {
  const controller = useController();
  const online = useOnline();
  const [inspection, setInspection] = useState(props.inspection);
  const [phase, setPhase] = useState(() => phaseOf(props.inspection));
  const owners = inspection.github?.available ? inspection.github.owners : [];
  const [owner, setOwner] = useState(owners[0]?.login ?? "");
  const [name, setName] = useState(inspection.suggested_name);
  const [check, setCheck] = useState<{
    owner: string;
    name: string;
    verdict: Verdict;
  } | null>(null);
  // GitHub refused to create this owner/name (`publish_failed`, nothing created).
  const [refused, setRefused] = useState<{ owner: string; name: string } | null>(null);
  // GitHub created these `owner/name`s before the push failed: taken, whatever a check answers.
  const [created, setCreated] = useState<readonly string[]>([]);
  // Offered once a publish failed: adding the folder needs no GitHub.
  const [plainOffered, setPlainOffered] = useState(false);
  // The add sent and not failed: with a GitHub repository, or without.
  const [sending, setSending] = useState<"publish" | "plain" | null>(null);
  const [inspecting, setInspecting] = useState(false);
  const [added, setAdded] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Every change asks again; an answer counts only if no newer question was asked.
  const asked = useRef(0);
  // Set from the first click until the add fails: a second click or Enter sends nothing.
  const adding = useRef(false);
  const field = useRef<HTMLInputElement>(null);
  // The root ids listed when Create was clicked: a root the add makes is one not among them.
  // Null when the list couldn't be loaded: then no root can be told apart as new.
  const rootsBefore = useRef<readonly string[] | null>([]);
  // Stops the watch (`watching`) at once: Cancel, Escape and the backdrop close the dialog.
  const stopWatching = useRef(() => {});
  const onAddedRef = useRef(onAdded);
  onAddedRef.current = onAdded;
  const nameId = useId();
  const busy = sending !== null;
  const rule = nameRule(name);
  const createdHere = created.includes(`${owner}/${name}`);
  const verdict = createdHere ? "taken" : check && check.owner === owner && check.name === name ? check.verdict : null;
  const refusedHere = refused !== null && refused.owner === owner && refused.name === name;
  const ready = online && !busy && owner !== "" && rule === null && verdict === "available";
  const large = inspection.first_commit?.large ? inspection.first_commit : null;
  const branch = inspection.branch ?? "main";
  const hostName = remote ? "the host" : "this Mac";
  // While the Host may still be adding the folder, or didn't finish, the steps and the large line are behind it.
  const waiting = phase.kind === "watching" || phase.kind === "unfinished";

  function close() {
    stopWatching.current();
    onClose();
  }

  useEffect(() => {
    if (phase.kind !== "form") return;
    field.current?.focus();
    field.current?.select();
  }, [phase.kind]);

  useEffect(() => {
    const id = ++asked.current;
    if (phase.kind !== "form" || rule !== null || owner === "" || createdHere) return;
    const timer = setTimeout(() => {
      const answer = (verdict: Verdict) => {
        if (asked.current === id) setCheck({ owner, name, verdict });
      };
      controller.repoAvailable(owner, name).then(
        (free) => answer(free ? "available" : "taken"),
        () => answer("failed"),
      );
    }, CHECK_DELAY_MS);
    return () => clearTimeout(timer);
  }, [controller, owner, name, rule, phase.kind, createdHere]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !busy) close();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose, busy]);

  // The add outlasted the SDK's timeout, so the Host may still be adding the
  // folder: read it every 10 s, one read at a time, for 10 minutes. Once it
  // has an origin and its root is listed, the add is done: go on as if it had
  // answered. The subject is fixed for the dialog's life (the Sidebar keys it).
  useEffect(() => {
    if (phase.kind !== "watching") return;
    let stopped = false;
    let next: ReturnType<typeof setTimeout> | undefined;
    const stop = () => {
      stopped = true;
      clearTimeout(next);
      clearTimeout(limit);
    };
    const limit = setTimeout(() => {
      stop();
      setPhase({ kind: "unfinished" });
    }, WATCH_FOR_MS);
    const read = async () => {
      try {
        const now =
          subject.kind === "path"
            ? await controller.inspectWorkspace(subject.path)
            : await controller.inspectRoot(subject.workspace_id);
        if (stopped || now.git !== "origin") return;
        await controller.refreshWorkspaces();
        if (stopped) return;
        const roots = (controller.store.getState().workspaces ?? []).filter((w) => w.kind === "root");
        const before = rootsBefore.current;
        const fresh = before === null ? [] : roots.filter((w) => !before.includes(w.workspace_id));
        // A picked symlink registers under its target's name: then the one new root is it.
        const root =
          subject.kind === "root"
            ? roots.find((w) => w.workspace_id === subject.workspace_id)
            : (fresh.find((w) => w.name === folderName(subject.path)) ?? (fresh.length === 1 ? fresh[0] : undefined));
        if (!root) return;
        stop();
        setSending("publish");
        setAdded(true);
        onAddedRef.current(root);
      } catch {
        // A read that fails (offline, a slow Host) is not the add failing: read again.
      } finally {
        if (!stopped) next = setTimeout(() => void read(), WATCH_EVERY_MS);
      }
    };
    next = setTimeout(() => void read(), WATCH_EVERY_MS);
    stopWatching.current = stop;
    return () => {
      stop();
      stopWatching.current = () => {};
    };
  }, [controller, phase.kind]);

  /**
   * Sends the add at most once at a time; `failed` maps the add's own error.
   * `first` runs once the add is claimed, before it is sent.
   */
  async function add(
    kind: "publish" | "plain",
    opts: AddOptions,
    failed: (e: unknown) => void,
    first?: () => Promise<void>,
  ): Promise<void> {
    if (adding.current) return;
    adding.current = true;
    setSending(kind);
    setError(null);
    await first?.();
    let workspace: WorkspaceInfo;
    try {
      workspace =
        subject.kind === "path"
          ? await controller.addWorkspace(subject.path, opts)
          : await controller.addRoot(subject.workspace_id, opts);
    } catch (e) {
      adding.current = false;
      setSending(null);
      failed(e);
      return;
    }
    setAdded(true);
    // Outside the try: only the add's own failure is an add error.
    onAdded(workspace);
  }

  /** The add found the folder past the size limit: show the large line; the next click sends `allow_large`. */
  function showLarge(e: unknown): boolean {
    const details = largeFolderDetails(e);
    if (!details) return false;
    setInspection((i) => ({ ...i, first_commit: { ...details, large: true } }));
    return true;
  }

  /** Records the root ids listed now, loading the list first if it isn't yet. */
  async function recordRoots() {
    const listed = () => controller.store.getState().workspaces;
    if (!listed()) await controller.refreshWorkspaces().catch(() => {});
    const workspaces = listed();
    rootsBefore.current = workspaces ? workspaces.filter((w) => w.kind === "root").map((w) => w.workspace_id) : null;
  }

  function create() {
    if (!ready) return;
    const target = { owner, name };
    const opts = { ...(large ? { allowLarge: true } : {}), publish: target };
    return add("publish", opts, (e) => {
      if (showLarge(e)) return;
      const failure = publishFailedDetails(e);
      const reason = UNAVAILABLE.find((code) => isClientError(e, code));
      if (failure?.created_on_github) {
        setCreated((c) => [...c, `${owner}/${name}`]);
        setPhase({ kind: "failed", ...target });
      } else if (failure) {
        setRefused(target);
        setPlainOffered(true);
      } else if (isClientError(e, "already_exists")) {
        setCheck({ ...target, verdict: "taken" });
      } else if (reason) {
        setPhase({ kind: "unavailable", reason });
      } else if (isClientError(e, "timeout") || isClientError(e, "unknown_outcome")) {
        // The Host may still be adding it (a large first commit, a slow push): watch, never resend.
        adding.current = true;
        setPhase({ kind: "watching" });
      } else {
        setError(errorMessage(e));
      }
    }, recordRoots);
  }

  /** The folder as it is, with no GitHub repository; `allow_large` when the large line showed. */
  function addWithoutGithub() {
    if (!online || busy) return;
    return add("plain", large ? { allowLarge: true } : {}, (e) => {
      if (!showLarge(e)) setError(errorMessage(e));
    });
  }

  /** Reads the folder again (the unavailable form's Try again); never adds. */
  async function inspectAgain() {
    if (!online || busy || inspecting) return;
    setInspecting(true);
    setError(null);
    try {
      const next =
        subject.kind === "path"
          ? await controller.inspectWorkspace(subject.path)
          : await controller.inspectRoot(subject.workspace_id);
      const nextOwners = next.github?.available ? next.github.owners : [];
      setInspection(next);
      setPhase(phaseOf(next));
      setOwner((o) => (nextOwners.some((n) => n.login === o) ? o : (nextOwners[0]?.login ?? "")));
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setInspecting(false);
    }
  }

  /** Back to the form after the push failed: the name GitHub now holds stays taken (`created`). */
  function tryAnotherName() {
    setError(null);
    setPhase({ kind: "form" });
  }

  const addPlainButton = (variant: "primary" | "secondary", label = "Add without GitHub") => (
    <Button
      variant={variant === "secondary" ? "secondary" : undefined}
      className={variant === "primary" ? PRIMARY : undefined}
      size="sm"
      disabled={!online || busy || inspecting}
      onClick={() => void addWithoutGithub()}
    >
      {sending === "plain" && <Loader2 className="animate-spin" />}
      {sending === "plain" && added ? "Preparing workspace…" : label}
    </Button>
  );

  const title = "Create a private GitHub repo?";
  const cancel = (
    <Button variant="secondary" size="sm" disabled={busy} onClick={close}>
      Cancel
    </Button>
  );
  return (
    <div
      className="fixed inset-0 z-50 grid place-items-center bg-black/40 p-6"
      onPointerDown={(e) => {
        if (e.target === e.currentTarget && !busy) close();
      }}
    >
      <div
        className="bg-card text-card-foreground ring-border flex w-full max-w-lg flex-col rounded-2xl shadow-2xl ring-1"
        role="dialog"
        aria-modal="true"
        aria-label={title}
      >
        <div className="flex flex-col gap-2 px-5 pt-5">
          <h2 className="text-base font-semibold tracking-tight">{title}</h2>
          {/* A detached HEAD has no branch to name; a folder with a remote gets no GitHub steps. */}
          {phase.kind !== "detached" && phase.kind !== "has_origin" && !waiting && (
            <p className="text-muted-foreground text-sm leading-relaxed">
              {inspection.git === "none" ? (
                <>
                  Zwiim will run <Code>git init</Code>, create a private GitHub repo, add it as <Code>origin</Code>, and
                  push <Code>{branch}</Code>.
                </>
              ) : (
                <>
                  Zwiim will {inspection.first_commit ? "make a first commit, create" : "create"} a private GitHub repo,
                  add it as <Code>origin</Code>, and push <Code>{branch}</Code>.
                </>
              )}
            </p>
          )}
        </div>
        <div className="flex flex-col gap-3 px-5 pt-4">
          {phase.kind === "form" ? (
            <>
              <div className="flex flex-col gap-1">
                <span className="text-xs font-medium">Owner</span>
                <ControlSelect
                  className="bg-input/30 text-foreground h-8 w-full justify-between rounded-md border"
                  label="Owner"
                  value={owner}
                  disabled={busy}
                  onChange={setOwner}
                  options={owners.map((o) => ({ value: o.login, label: o.login }))}
                />
              </div>
              <div className="flex flex-col gap-1">
                <label className="text-xs font-medium" htmlFor={nameId}>
                  Repository name
                </label>
                <input
                  ref={field}
                  id={nameId}
                  className="bg-input/30 w-full rounded-md border px-2 py-1.5 font-mono text-xs outline-none"
                  value={name}
                  disabled={busy}
                  spellCheck={false}
                  autoComplete="off"
                  aria-invalid={rule !== null || verdict === "taken" || refusedHere}
                  onChange={(e) => setName(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" && !e.nativeEvent.isComposing) {
                      e.preventDefault();
                      void create();
                    }
                  }}
                />
                <p className="text-muted-foreground font-mono text-xs">
                  Will create {owner}/{name}
                </p>
                <p
                  className={cn(
                    "text-xs",
                    rule !== null || verdict === "taken" || refusedHere
                      ? "text-destructive"
                      : verdict === "available"
                        ? "text-done"
                        : "text-muted-foreground",
                  )}
                  aria-live="polite"
                >
                  {rule ??
                    (refusedHere ? (
                      <>
                        GitHub didn't create{" "}
                        <Code>
                          {owner}/{name}
                        </Code>
                        . Try another owner or name.
                      </>
                    ) : verdict === "available" ? (
                      "Repository name is available."
                    ) : verdict === "taken" ? (
                      <>
                        <Code>
                          {owner}/{name}
                        </Code>{" "}
                        already exists.
                      </>
                    ) : verdict === "failed" ? (
                      "Couldn't check this name."
                    ) : (
                      "Checking…"
                    ))}
                </p>
              </div>
            </>
          ) : phase.kind === "failed" ? (
            <div className="flex flex-col items-start gap-2">
              <p className="text-sm">
                <Code>
                  {phase.owner}/{phase.name}
                </Code>{" "}
                was created on GitHub, but pushing to it failed.
              </p>
              <button
                type="button"
                className="text-brand text-xs underline-offset-2 hover:underline"
                onClick={() => void openLink(`https://github.com/${phase.owner}/${phase.name}`).catch(() => {})}
              >
                Open on GitHub
                <ArrowUpRight className="ml-0.5 inline size-3 align-[-1px]" aria-hidden />
              </button>
            </div>
          ) : phase.kind === "watching" ? (
            // Once the watch found the root, the button says what happens next.
            !added && (
              <p className="flex items-center gap-2 text-sm" role="status">
                <Loader2 className="text-muted-foreground size-4 animate-spin" aria-hidden />
                Still working on {hostName}…
              </p>
            )
          ) : phase.kind === "unfinished" ? (
            <p className="text-sm">
              {remote ? "The host" : "This Mac"} didn't finish adding this folder. Check the sidebar, or try again.
            </p>
          ) : (
            <p className="text-sm">
              {phase.kind === "unavailable"
                ? unavailableLine(phase.reason, hostName)
                : phase.kind === "has_origin"
                  ? "This folder already has a remote, so there's nothing to publish."
                  : `This repository isn't on a branch. Check out a branch on ${hostName}, then try again.`}
            </p>
          )}
          {large && !waiting && (
            <p className="text-muted-foreground text-xs">
              The first commit will hold <Code>{largeFolderText(large)}</Code>.
            </p>
          )}
        </div>
        <div className="flex flex-col gap-2 p-5">
          <div className="flex justify-end gap-2">
            {cancel}
            {phase.kind === "form" ? (
              <>
                {plainOffered && addPlainButton("secondary")}
                <Button className={PRIMARY} size="sm" disabled={!ready} onClick={() => void create()}>
                  {sending === "publish" && <Loader2 className="animate-spin" />}
                  {sending === "publish"
                    ? added
                      ? "Preparing workspace…"
                      : "Creating repo…"
                    : inspection.git === "none"
                      ? "Initialize Git and create repo"
                      : "Create repo"}
                </Button>
              </>
            ) : phase.kind === "failed" ? (
              <>
                <Button variant="secondary" size="sm" disabled={busy} onClick={tryAnotherName}>
                  Try another name
                </Button>
                {addPlainButton("primary")}
              </>
            ) : phase.kind === "has_origin" ? (
              // The remote may well be GitHub, and the add keeps it.
              addPlainButton("primary", "Add repository")
            ) : phase.kind === "watching" || phase.kind === "unfinished" ? (
              // Cancel only, until the watch finds the root and the worktree step runs.
              added && (
                <Button className={PRIMARY} size="sm" disabled>
                  <Loader2 className="animate-spin" />
                  Preparing workspace…
                </Button>
              )
            ) : (
              <>
                {addPlainButton("secondary")}
                <Button
                  className={PRIMARY}
                  size="sm"
                  disabled={!online || busy || inspecting}
                  onClick={() => void inspectAgain()}
                >
                  {inspecting && <Loader2 className="animate-spin" />}
                  Try again
                </Button>
              </>
            )}
          </div>
          {error && (
            <p className="text-destructive text-xs" role="alert">
              {error}
            </p>
          )}
        </div>
      </div>
    </div>
  );
}
