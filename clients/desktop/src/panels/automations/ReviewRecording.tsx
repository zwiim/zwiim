import { useState } from "react";
import { Loader2 } from "lucide-react";
import type { PlaybookActionKind, PlaybookDraft, PlaybookDraftStep, PlaybookSummary } from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import { errorMessage, useController } from "@/lib/host-context";

/** The sends a recording may be; delete, pay and account changes never come from one. */
const KINDS: { kind: PlaybookActionKind; label: string }[] = [
  { kind: "submit_application", label: "Apply" },
  { kind: "submit_form", label: "Submit a form" },
  { kind: "publish_post", label: "Post" },
  { kind: "send_message", label: "Send a message" },
  { kind: "comment", label: "Comment" },
  { kind: "follow", label: "Follow" },
];

const field =
  "border-border bg-input/30 text-ui focus-visible:ring-ring w-full rounded-[10px] border px-2.5 py-1.5 outline-none focus-visible:ring-2";

/** What a step did, as the person reads it. */
function stepText(step: PlaybookDraftStep): string {
  switch (step.value.kind) {
    case "my_info":
    case "fixed":
      return step.action === "choose" ? `Chose ${step.target}` : `Filled ${step.target}`;
    case "file":
      return `Attached a file to ${step.target}`;
    case "needs_you":
      return `${step.target}: you type this each time`;
    case "none":
      return step.action === "navigate" ? `Went to ${step.url}` : `Clicked ${step.target}`;
  }
}

/** Why the draft cannot be saved yet, if it cannot. */
function blocker(d: PlaybookDraft): string | null {
  if (d.send_step === null) return "Choose which step sends.";
  for (const s of d.steps) {
    if (s.value.kind === "file" && !s.value.path) return `Pick the file for ${s.value.input}.`;
  }
  if (!d.name.trim()) return "Give it a name.";
  return null;
}

/**
 * What a recording became, for the person to check before it is saved:
 * which values are their info, which step sends, the sites, and how to
 * tell it worked (recording design §6).
 */
export function ReviewRecording({
  draft: recorded,
  pickFile,
  onSaved,
  onClose,
}: {
  draft: PlaybookDraft;
  pickFile(): Promise<string | null>;
  /** Saved; `tryIt` when the person asked to try it on one item. */
  onSaved(playbook: PlaybookSummary, tryIt: boolean): void;
  onClose(): void;
}) {
  const controller = useController();
  const [d, setD] = useState(recorded);
  const [sites, setSites] = useState<Set<string>>(() => new Set(recorded.sites));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const why = blocker(d);

  const setStep = (i: number, step: PlaybookDraftStep) =>
    setD((x) => ({ ...x, steps: x.steps.map((s, j) => (j === i ? step : s)) }));

  async function pick(i: number) {
    const step = d.steps[i]!;
    if (step.value.kind !== "file") return;
    const path = await pickFile();
    if (path) setStep(i, { ...step, value: { ...step.value, path } });
  }

  async function save(tryIt: boolean) {
    setBusy(true);
    setError(null);
    try {
      const { playbook } = await controller.createPlaybookFromDraft({
        ...d,
        sites: d.sites.filter((s) => sites.has(s)),
        check_text: d.check_text?.trim() ? d.check_text.trim() : null,
      });
      onSaved(playbook, tryIt);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-y-auto px-6 py-4" data-testid="review-recording">
      <div className="mx-auto flex w-full max-w-2xl flex-col gap-5">
        <header className="flex flex-col gap-1">
          <h1 className="text-lg font-semibold tracking-tight">Check what Zwiim saw</h1>
          <p className="text-muted-foreground text-ui">
            The agent follows these steps as a guide on each item and finds the right field when a page differs.
          </p>
        </header>

        <label className="flex flex-col gap-1">
          <span className="text-muted-foreground text-xs">Name</span>
          <input className={field} value={d.name} onChange={(e) => setD({ ...d, name: e.target.value })} />
        </label>
        <label className="flex flex-col gap-1">
          <span className="text-muted-foreground text-xs">Goal</span>
          <input className={field} value={d.goal} onChange={(e) => setD({ ...d, goal: e.target.value })} />
        </label>

        <label className="flex flex-col gap-1">
          <span className="text-muted-foreground text-xs">
            {d.source.type === "search" ? "Where items come from (a list or search page)" : "Pages to work, one per line"}
          </span>
          {d.source.type === "search" ? (
            <input
              className={`${field} font-mono text-xs`}
              value={d.source.url}
              onChange={(e) => setD({ ...d, source: { type: "search", url: e.target.value } })}
            />
          ) : (
            <textarea
              className={`${field} min-h-16 font-mono text-xs`}
              value={d.source.urls.join("\n")}
              onChange={(e) =>
                setD({
                  ...d,
                  source: {
                    type: "list",
                    urls: e.target.value
                      .split("\n")
                      .map((u) => u.trim())
                      .filter(Boolean),
                  },
                })
              }
            />
          )}
        </label>

        <fieldset className="flex flex-col gap-1">
          <legend className="text-muted-foreground pb-1 text-xs">Steps · mark the one that sends</legend>
          {d.steps.map((step, i) => (
            <div key={i} className="flex items-center gap-2 py-0.5" data-testid={`review-step-${i}`}>
              <input
                type="radio"
                name="send"
                aria-label={`${step.target || stepText(step)} sends`}
                checked={d.send_step === i}
                disabled={step.action !== "click"}
                onChange={() => setD({ ...d, send_step: i })}
              />
              <span className="text-ui min-w-0 flex-1 truncate">{stepText(step)}</span>
              {(step.value.kind === "my_info" || step.value.kind === "fixed") && (
                <label className="text-muted-foreground flex items-center gap-1.5 text-xs">
                  <input
                    type="checkbox"
                    aria-label={`${step.target} is my info`}
                    checked={step.value.kind === "my_info"}
                    onChange={(e) => {
                      const v = step.value;
                      const text = v.kind === "my_info" ? v.value : v.kind === "fixed" ? v.text : "";
                      // Back to info: the input name the Host gave it, or its field's.
                      const was = recorded.steps[i]?.value;
                      const input = was?.kind === "my_info" ? was.input : step.target || "Info";
                      setStep(i, {
                        ...step,
                        value: e.target.checked ? { kind: "my_info", input, value: text } : { kind: "fixed", text },
                      });
                    }}
                  />
                  My info
                </label>
              )}
              {step.value.kind === "file" && (
                <>
                  <span className="text-muted-foreground max-w-48 truncate font-mono text-xs">
                    {step.value.path ? step.value.path.split(/[\\/]/).pop() : "No file yet"}
                  </span>
                  <Button
                    variant="secondary"
                    size="xs"
                    aria-label={`Pick ${step.value.input}`}
                    onClick={() => void pick(i)}
                  >
                    {step.value.path ? "Change" : "Pick file"}
                  </Button>
                </>
              )}
            </div>
          ))}
        </fieldset>

        <label className="flex flex-col gap-1">
          <span className="text-muted-foreground text-xs">What the send does</span>
          <select
            className={field}
            value={d.send_kind}
            onChange={(e) => setD({ ...d, send_kind: e.target.value as PlaybookActionKind })}
          >
            {KINDS.map((k) => (
              <option key={k.kind} value={k.kind}>
                {k.label}
              </option>
            ))}
          </select>
        </label>

        <fieldset className="flex flex-col gap-1">
          <legend className="text-muted-foreground pb-1 text-xs">Sites it may use</legend>
          {d.sites.map((s) => (
            <label key={s} className="text-ui flex items-center gap-2">
              <input
                type="checkbox"
                checked={sites.has(s)}
                onChange={(e) =>
                  setSites((x) => {
                    const next = new Set(x);
                    if (e.target.checked) next.add(s);
                    else next.delete(s);
                    return next;
                  })
                }
              />
              <span className="font-mono text-xs">{s}</span>
            </label>
          ))}
        </fieldset>

        <label className="flex flex-col gap-1">
          <span className="text-muted-foreground text-xs">Text that shows it worked</span>
          <input
            className={field}
            aria-label="Text that shows it worked"
            placeholder="Application sent"
            value={d.check_text ?? ""}
            onChange={(e) => setD({ ...d, check_text: e.target.value })}
          />
          <span className="text-muted-foreground text-xs">
            Optional. Without it, the agent judges from the page whether the send went through.
          </span>
        </label>

        {(why || error) && (
          <p className={error ? "text-destructive text-xs" : "text-muted-foreground text-xs"} role={error ? "alert" : undefined}>
            {error ?? why}
          </p>
        )}
        <div className="flex justify-end gap-2 pb-4">
          <Button variant="ghost" size="sm" disabled={busy} onClick={onClose}>
            Discard
          </Button>
          <Button variant="secondary" size="sm" disabled={busy || why !== null} onClick={() => void save(false)}>
            Save
          </Button>
          <Button variant="brand" size="sm" disabled={busy || why !== null} onClick={() => void save(true)}>
            {busy && <Loader2 className="animate-spin" />}
            Save and try on one item
          </Button>
        </div>
      </div>
    </div>
  );
}
