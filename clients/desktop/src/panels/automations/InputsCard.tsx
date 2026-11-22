import { useCallback, useEffect, useState } from "react";
import type { PlaybookFact, PlaybookFile } from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import { errorMessage, useController } from "@/lib/host-context";

const IN_USE = "A step still uses this file. Remove that step or replace the file.";

/** `240 KB`, `3.1 MB`. */
function sizeText(bytes: number): string {
  if (bytes < 1024 * 1024) return `${Math.max(1, Math.round(bytes / 1024))} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/** `22 Nov`. */
function dayText(iso: string): string {
  return new Date(iso).toLocaleDateString("en-GB", { day: "numeric", month: "short" });
}

/** What a file row says about its file. */
function fileText(f: PlaybookFile): string {
  if (!f.file) return `On this computer: ${f.path ?? ""}`;
  const parts = [f.file];
  if (f.size !== null) parts.push(sizeText(f.size));
  if (f.added_at) parts.push(`added ${dayText(f.added_at)}`);
  return parts.join(" · ");
}

/** The input name a new file gets: its own name without the extension. */
function nameFrom(path: string): string {
  const base = path.split(/[\\/]/).pop() ?? path;
  const dot = base.lastIndexOf(".");
  return (dot > 0 ? base.slice(0, dot) : base) || "File";
}

/**
 * A playbook's inputs: the files the Host keeps for it and the person's
 * info. A change saves a new version; items still waiting use it.
 */
export function InputsCard({
  playbookId,
  pickFile,
}: {
  playbookId: string;
  /** A file on the Host's computer, or null when the person cancelled. */
  pickFile(): Promise<string | null>;
}) {
  const controller = useController();
  const [files, setFiles] = useState<PlaybookFile[] | null>(null);
  const [facts, setFacts] = useState<PlaybookFact[]>([]);
  const [editing, setEditing] = useState<{ name: string; value: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(() => {
    controller.listPlaybookInputs(playbookId).then(
      (r) => {
        setFiles(r.files);
        setFacts(r.facts);
      },
      (e) => setError(errorMessage(e)),
    );
  }, [controller, playbookId]);
  useEffect(load, [load]);

  async function act(op: () => Promise<unknown>) {
    setBusy(true);
    setError(null);
    try {
      await op();
      load();
    } catch (e) {
      const code = e && typeof e === "object" && "code" in e ? (e as { code: unknown }).code : null;
      setError(code === "input_in_use" ? IN_USE : errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  async function putFile(name: string | null) {
    const path = await pickFile();
    if (!path) return;
    await act(() => controller.setPlaybookFile(playbookId, name ?? nameFrom(path), path));
  }

  if (files === null) return null;

  return (
    <div className="flex flex-col gap-2" data-testid="inputs-card">
      <span className="text-muted-foreground text-xs font-medium">Inputs</span>
      {files.length === 0 && facts.length === 0 && (
        <p className="text-muted-foreground text-xs">No inputs. Add a file the playbook attaches.</p>
      )}
      {files.map((f) => (
        <div key={f.input_name} className="flex items-center gap-2" data-testid={`input-file-${f.input_name}`}>
          <span className="text-ui shrink-0">{f.input_name}</span>
          <span className="text-muted-foreground min-w-0 flex-1 truncate font-mono text-xs">{fileText(f)}</span>
          <Button
            variant="ghost"
            size="xs"
            disabled={busy}
            aria-label={`Replace ${f.input_name}`}
            onClick={() => void putFile(f.input_name)}
          >
            Replace
          </Button>
          <Button
            variant="ghost"
            size="xs"
            disabled={busy}
            aria-label={`Remove ${f.input_name}`}
            onClick={() => void act(() => controller.removePlaybookFile(playbookId, f.input_name))}
          >
            Remove
          </Button>
        </div>
      ))}
      {facts.map((f) => (
        <div key={f.input_name} className="flex items-center gap-2" data-testid={`input-fact-${f.input_name}`}>
          <span className="text-ui shrink-0">{f.input_name}</span>
          {editing?.name === f.input_name ? (
            <form
              className="flex min-w-0 flex-1 items-center gap-2"
              onSubmit={(e) => {
                e.preventDefault();
                const { name, value } = editing;
                setEditing(null);
                void act(() => controller.setPlaybookFact(playbookId, name, value));
              }}
            >
              <input
                className="border-border bg-input/30 text-ui focus-visible:ring-ring min-w-0 flex-1 rounded-[10px] border px-2 py-1 outline-none focus-visible:ring-2"
                aria-label={f.input_name}
                autoFocus
                value={editing.value}
                onChange={(e) => setEditing({ name: f.input_name, value: e.target.value })}
                onKeyDown={(e) => e.key === "Escape" && setEditing(null)}
              />
              <Button type="submit" variant="secondary" size="xs" disabled={busy}>
                Save
              </Button>
            </form>
          ) : (
            <>
              <span className="text-muted-foreground min-w-0 flex-1 truncate text-xs">{f.value}</span>
              <Button
                variant="ghost"
                size="xs"
                disabled={busy}
                aria-label={`Edit ${f.input_name}`}
                onClick={() => setEditing({ name: f.input_name, value: f.value })}
              >
                Edit
              </Button>
            </>
          )}
        </div>
      ))}
      <div>
        <Button variant="secondary" size="xs" disabled={busy} onClick={() => void putFile(null)}>
          Add file
        </Button>
      </div>
      {error && (
        <p className="text-destructive text-xs" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}
