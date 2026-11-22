import { createContext, useCallback, useContext, useMemo, useState } from "react";
import { tabRecording, type PlaybookDraft } from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import { errorMessage, useController, useHost } from "@/lib/host-context";

const NO_WORKSPACE = "Add a project folder first: recording happens in the Browser panel beside it.";

/**
 * Recording a playbook (recording design 2026-11-22 §6): one tab of the
 * person's own records what they do; Stop turns it into a draft to review.
 * The Host keeps the recording; this holds which tab records and the draft.
 */
export interface RecordingApi {
  /** The tab recording now, if any. */
  tabId: string | null;
  /** The draft waiting for review, if any. */
  review: { draft: PlaybookDraft } | null;
  /** Grows each time the Browser panel should come forward. */
  showBrowser: number;
  error: string | null;
  /** Open a new tab of one's own and record in it. */
  record(): Promise<void>;
  /** Record in tab `tabId`, one of one's own. */
  start(tabId: string): Promise<void>;
  /** Stop, and review what was recorded. */
  stop(): Promise<void>;
  /** Stop and drop it. */
  cancel(): Promise<void>;
  closeReview(): void;
}

const RecordingContext = createContext<RecordingApi | null>(null);

export function useRecording(): RecordingApi | null {
  return useContext(RecordingContext);
}

export function RecordingProvider({
  canShowBrowser,
  onShowBrowser,
  onReview,
  children,
}: {
  /** A workspace is on screen, so its Browser panel can show. */
  canShowBrowser: boolean;
  onShowBrowser(): void;
  onReview(): void;
  children: React.ReactNode;
}) {
  const controller = useController();
  const [tabId, setTabId] = useState<string | null>(null);
  const [review, setReview] = useState<{ draft: PlaybookDraft } | null>(null);
  const [showBrowser, setShowBrowser] = useState(0);
  const [error, setError] = useState<string | null>(null);

  const start = useCallback(
    async (id: string) => {
      setError(null);
      try {
        await controller.startRecording(id);
        setTabId(id);
      } catch (e) {
        setError(errorMessage(e));
      }
    },
    [controller],
  );

  const record = useCallback(async () => {
    setError(null);
    if (!canShowBrowser) return setError(NO_WORKSPACE);
    try {
      const tab = await controller.openBrowserTab({ kind: "personal" });
      await controller.startRecording(tab.tab_id);
      setTabId(tab.tab_id);
      setShowBrowser((n) => n + 1);
      onShowBrowser();
    } catch (e) {
      setError(errorMessage(e));
    }
  }, [canShowBrowser, controller, onShowBrowser]);

  const end = useCallback(
    async (keep: boolean) => {
      if (!tabId) return;
      setError(null);
      try {
        const { draft } = await controller.stopRecording(tabId);
        if (keep) {
          setReview({ draft });
          onReview();
        }
      } catch (e) {
        setError(errorMessage(e));
      } finally {
        setTabId(null);
      }
    },
    [controller, onReview, tabId],
  );

  const api = useMemo<RecordingApi>(
    () => ({
      tabId,
      review,
      showBrowser,
      error,
      record,
      start,
      stop: () => end(true),
      cancel: () => end(false),
      closeReview: () => setReview(null),
    }),
    [tabId, review, showBrowser, error, record, start, end],
  );
  return <RecordingContext.Provider value={api}>{children}</RecordingContext.Provider>;
}

/** Over a recording tab: how far it got, with Stop and Cancel. */
export function RecordingBar({ tabId }: { tabId: string }) {
  const recording = useRecording();
  const progress = useHost((s) => tabRecording(s.browser, tabId));
  const [busy, setBusy] = useState(false);
  if (!recording || recording.tabId !== tabId) return null;
  const count = progress?.count ?? 0;

  async function end(op: () => Promise<void>) {
    setBusy(true);
    try {
      await op();
    } finally {
      setBusy(false);
    }
  }

  return (
    <div
      className="bg-card flex items-center gap-2 border-b px-3 py-1.5"
      data-testid="recording-bar"
      role="status"
    >
      <span className="bg-destructive size-2 shrink-0 animate-pulse rounded-full" aria-hidden />
      <span className="text-ui font-medium">Recording</span>
      <span className="text-muted-foreground font-mono text-xs tabular-nums">
        {count} {count === 1 ? "step" : "steps"}
      </span>
      {progress?.last.name && (
        <span className="text-muted-foreground min-w-0 flex-1 truncate text-xs">· {progress.last.name}</span>
      )}
      <span className="flex-1" />
      <Button variant="ghost" size="xs" disabled={busy} onClick={() => void end(recording.cancel)}>
        Cancel
      </Button>
      <Button variant="brand" size="xs" disabled={busy} onClick={() => void end(recording.stop)}>
        Stop
      </Button>
    </div>
  );
}
