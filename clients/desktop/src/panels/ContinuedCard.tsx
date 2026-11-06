import type { AttachmentBlock, ForkWorktree, PlanContinues } from "@zwiim/sdk";
import { ArrowRight } from "lucide-react";

import { SentChip } from "@/attachments/AttachmentChip";
import { useHost } from "@/lib/host-context";

/** A session's title as the store lists it, in any workspace; null when it is not listed. */
function useSessionTitle(sessionId: string): string | null | undefined {
  return useHost((s) => {
    for (const list of Object.values(s.sessionsByWorkspace)) {
      const found = list.find((x) => x.session_id === sessionId);
      if (found) return found.title || null;
    }
    return undefined;
  });
}

/**
 * A session started to finish another's plan: its first message, which is the
 * long prompt the Host wrote, shows as this card. The prompt itself is not shown.
 */
export function ContinuedCard({
  continues,
  attachments,
  onOpenSession,
}: {
  continues: PlanContinues;
  /** The first message's blocks; only its attachments show, as chips. */
  attachments: readonly { type: string }[];
  onOpenSession(id: string): void;
}) {
  const agentName = useHost((s) =>
    continues.agent ? (s.agents?.find((a) => a.agent === continues.agent)?.name ?? continues.agent) : null,
  );
  const listed = useSessionTitle(continues.session_id);
  const title = continues.title || listed || "the previous session";
  const counted = continues.completed !== undefined && continues.total !== undefined;
  const files = attachments.filter((b): b is AttachmentBlock => b.type === "attachment");
  return (
    <div className="bg-card flex flex-col gap-2 rounded-xl px-4 py-3" data-testid="continued-card">
      <p className="text-ui font-medium">
        {agentName ? `Continuing ${agentName}'s tasks` : "Continuing the tasks"}
        {counted && (
          <>
            {" · "}
            <span className="font-mono tabular-nums">
              {continues.completed} of {continues.total} done
            </span>
          </>
        )}
      </p>
      {files.length > 0 && (
        <div className="flex flex-wrap gap-1.5">
          {files.map((b) => (
            <SentChip key={b.attachment_id} block={b} />
          ))}
        </div>
      )}
      {listed !== undefined ? (
        <button
          type="button"
          className="text-muted-foreground hover:text-foreground self-start text-xs underline-offset-2 transition-colors hover:underline"
          onClick={() => onOpenSession(continues.session_id)}
        >
          From {title}
        </button>
      ) : (
        <p className="text-muted-foreground text-xs">From {title}</p>
      )}
    </div>
  );
}

/** Where this session's plan went: one row in place of the resume bar. */
export function ContinuedInRow({ sessionId, onOpenSession }: { sessionId: string; onOpenSession(id: string): void }) {
  const title = useSessionTitle(sessionId);
  return (
    <button
      type="button"
      className="text-muted-foreground hover:text-foreground flex items-center gap-1 self-start text-xs transition-colors"
      aria-label={`Continued in ${title || "new session"} →`}
      data-testid="continued-in"
      onClick={() => onOpenSession(sessionId)}
    >
      Continued in <span className="text-foreground/90 truncate font-medium">{title || "new session"}</span>
      <ArrowRight className="size-3 shrink-0" />
    </button>
  );
}

/** Above a forked session's chat: where it came from, a link while that session is listed. */
export function ForkedFromRow({
  from,
  onOpenSession,
}: {
  from: { session_id: string; title?: string | null };
  onOpenSession?(workspaceId: string, id: string): void;
}) {
  const workspaceId = useHost((s) => {
    for (const [id, list] of Object.entries(s.sessionsByWorkspace)) {
      if (list.some((x) => x.session_id === from.session_id)) return id;
    }
    return null;
  });
  const listed = useSessionTitle(from.session_id);
  const label = `Forked from ${from.title || listed || "a session that is gone"}`;
  return (
    <p className="text-muted-foreground text-xs" data-testid="forked-from">
      {workspaceId !== null && onOpenSession ? (
        <button
          type="button"
          className="hover:text-foreground underline-offset-2 transition-colors hover:underline"
          onClick={() => onOpenSession(workspaceId, from.session_id)}
        >
          {label}
        </button>
      ) : (
        label
      )}
    </p>
  );
}

/** A fork nobody has written in yet: the worktree it made, or where the new chat is. */
export function ForkStart({ workspaceId, worktree }: { workspaceId: string; worktree?: ForkWorktree | null }) {
  const name = useHost((s) => s.workspaces?.find((w) => w.workspace_id === workspaceId)?.name);
  if (!worktree) {
    return (
      <p className="text-muted-foreground py-10 text-center" data-testid="fork-start">
        New chat{name && <> in <span className="text-foreground/90 font-mono">{name}</span></>}
      </p>
    );
  }
  const copied = `${worktree.files.toLocaleString("en-US")} ${worktree.files === 1 ? "file" : "files"}`;
  return (
    <div className="bg-card flex flex-col gap-1 rounded-xl px-4 py-3" data-testid="fork-worktree">
      <p className="text-ui font-medium">
        You're in a new copy of {worktree.repository} called <span className="font-mono">{worktree.name}</span>
      </p>
      <p className="text-muted-foreground text-xs">
        {worktree.branch ? (
          <>
            Created <span className="font-mono">{worktree.branch}</span> and copied{" "}
          </>
        ) : (
          "Copied "
        )}
        <span className="font-mono tabular-nums">{copied}</span>
      </p>
    </div>
  );
}
