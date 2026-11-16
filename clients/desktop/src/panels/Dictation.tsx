import { useContext, useEffect, useRef, useState, type RefObject, type CSSProperties } from "react";
import { Check, Mic, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { VoiceContext, type VoiceBridge } from "@/lib/bridge";
import { cleanDictation, insertAtCaret } from "@/lib/dictation";
import { errorMessage } from "@/lib/host-context";
import { formatShortcut, isModKey } from "@/lib/platform";
import { usePresence } from "@/lib/presence";
import { voice, type VoicePrefs } from "@/lib/prefs";
import { usePref } from "@/lib/use-pref";
import { useOpenSettings } from "@/settings/open";

type Phase = "idle" | "starting" | "listening" | "stopping";

export function Dictation(props: {
  area: RefObject<HTMLTextAreaElement | null>;
  value: string;
  onChange(value: string): void;
  disabled?: boolean;
}) {
  const bridge = useContext(VoiceContext);
  const enabled = usePref(() => voice().enabled);
  return bridge && enabled ? <DictationControl {...props} bridge={bridge} /> : null;
}

function DictationControl({
  bridge,
  area,
  value,
  onChange,
  disabled,
}: {
  bridge: VoiceBridge;
  area: RefObject<HTMLTextAreaElement | null>;
  value: string;
  onChange(value: string): void;
  disabled?: boolean;
}) {
  const openSettings = useOpenSettings();
  const [phase, setPhase] = useState<Phase>("idle");
  const currentPhase = useRef<Phase>("idle");
  const [error, setError] = useState<string | null>(null);
  const [partial, setPartial] = useState("");
  const [rms, setRms] = useState(0);
  const [elapsed, setElapsed] = useState(0);
  const [ready, setReady] = useState(false);
  const live = useRef(false);
  const starting = useRef<Promise<void>>(Promise.resolve());
  const prefs = useRef<VoicePrefs>(voice());
  const began = useRef(0);
  const started = useRef(false);
  const discard = useRef(false);
  const latest = useRef({ value, onChange, disabled });
  latest.current = { value, onChange, disabled };
  const pressed = useRef<{
    time: number;
    mode: VoicePrefs["mode"];
    source: "key" | "pointer";
    started: boolean;
  } | null>(null);
  const presence = usePresence(phase !== "idle" || error !== null);
  const finishRef = useRef<(cancel?: boolean, message?: string) => Promise<void>>(async () => {});

  function changePhase(next: Phase) {
    currentPhase.current = next;
    if (live.current) setPhase(next);
  }

  async function finish(cancel = false, message?: string) {
    if (currentPhase.current === "idle") return;
    if (currentPhase.current === "stopping") {
      if (cancel) discard.current = true;
      return;
    }
    discard.current = cancel;
    changePhase("stopping");
    pressed.current = null;
    try {
      await starting.current;
      if (!started.current) return;
      if (cancel) await bridge.cancel();
      else {
        const text = cleanDictation(await bridge.stop(), prefs.current);
        if (live.current && text && !discard.current) {
          const el = area.current;
          const result = insertAtCaret(
            latest.current.value,
            el?.selectionStart ?? latest.current.value.length,
            el?.selectionEnd ?? latest.current.value.length,
            text,
          );
          latest.current.onChange(result.value);
          requestAnimationFrame(() => {
            if (!live.current) return;
            area.current?.focus();
            area.current?.setSelectionRange(result.caret, result.caret);
          });
        } else if (live.current) area.current?.focus();
      }
      if (live.current) {
        setError(message ?? null);
        if (cancel) area.current?.focus();
      }
    } catch (e) {
      if (live.current) setError(errorMessage(e));
    } finally {
      changePhase("idle");
    }
  }
  finishRef.current = finish;

  useEffect(() => {
    live.current = true;
    const off: (() => void)[] = [];
    let active = true;
    // Subscribe before opening the microphone, so an early stop cannot be missed.
    const subscriptions = [
      bridge.listen("level", (p) => {
        if (active && currentPhase.current === "listening") setRms(p.rms);
      }),
      bridge.listen("partial", (p) => {
        if (active && currentPhase.current === "listening") setPartial(p.text);
      }),
      bridge.listen("stopped", (p) => {
        if (active) void finishRef.current(p.reason === "error", p.error);
      }),
    ].map((promise) =>
      promise.then((unsubscribe) => {
        if (active) off.push(unsubscribe);
        else unsubscribe();
      }),
    );
    void Promise.all(subscriptions)
      .then(() => {
        if (active) setReady(true);
      })
      .catch((e) => {
        if (active) setError(errorMessage(e));
      });
    return () => {
      active = false;
      live.current = false;
      off.forEach((unsubscribe) => unsubscribe());
      if (currentPhase.current !== "idle") void starting.current.then(() => bridge.cancel()).catch(() => {});
    };
  }, [bridge]);

  useEffect(() => {
    if (phase !== "listening") return;
    const timer = window.setInterval(() => setElapsed(Math.floor((performance.now() - began.current) / 1000)), 250);
    return () => window.clearInterval(timer);
  }, [phase]);
  useEffect(() => {
    if (disabled) void finishRef.current(true);
  }, [disabled]);

  function start() {
    if (!ready || latest.current.disabled || currentPhase.current !== "idle") return;
    prefs.current = voice();
    setError(null);
    setPartial("");
    setRms(0);
    setElapsed(0);
    changePhase("starting");
    started.current = false;
    discard.current = false;
    starting.current = (async () => {
      try {
        await bridge.unloadAfter(prefs.current.keepLoadedSecs);
        if (!live.current) return;
        const { model, language, input, gpu, silenceSecs } = prefs.current;
        await bridge.start({
          model,
          language,
          input,
          gpu,
          silence_secs: silenceSecs,
        });
        started.current = true;
        began.current = performance.now();
        if (currentPhase.current === "starting") changePhase("listening");
      } catch (e) {
        if (live.current) setError(errorMessage(e));
        changePhase("idle");
      }
    })();
  }

  function press(source: "key" | "pointer") {
    if (pressed.current || latest.current.disabled) return;
    const mode = voice().mode;
    const started = currentPhase.current === "idle";
    pressed.current = { time: performance.now(), mode, source, started };
    if (started) start();
    else if (mode !== "hold") void finish();
  }
  function release(source: "key" | "pointer") {
    const p = pressed.current;
    if (!p || p.source !== source) return;
    pressed.current = null;
    if (p.started && (p.mode === "hold" || (p.mode === "auto" && performance.now() - p.time > 400))) void finish();
  }
  const handlers = useRef({ press, release });
  handlers.current = { press, release };
  useEffect(() => {
    const eligible = (target: EventTarget | null) => {
      if (document.querySelector('[role="dialog"]')) return false;
      const focused = target instanceof Node ? target : document.activeElement;
      return (
        focused === document ||
        focused === document.body ||
        focused === document.documentElement ||
        (!!focused && !!area.current?.closest('[data-testid="composer"]')?.contains(focused))
      );
    };
    const down = (e: KeyboardEvent) => {
      if (e.key === "Escape" && currentPhase.current !== "idle") {
        e.preventDefault();
        e.stopPropagation();
        void finishRef.current(true);
        return;
      }
      if (e.key.toLowerCase() !== "e" || !isModKey(e) || e.altKey || e.shiftKey || !eligible(e.target)) return;
      e.preventDefault();
      e.stopPropagation();
      if (!e.repeat) handlers.current.press("key");
    };
    const up = (e: KeyboardEvent) => {
      if (e.key.toLowerCase() === "e" || e.key === "Meta" || e.key === "Control") handlers.current.release("key");
    };
    const blur = () => {
      const p = pressed.current;
      if (p && (p.mode === "hold" || (p.mode === "auto" && performance.now() - p.time > 400))) void finishRef.current();
      pressed.current = null;
    };
    window.addEventListener("keydown", down, true);
    window.addEventListener("keyup", up, true);
    window.addEventListener("blur", blur);
    return () => {
      window.removeEventListener("keydown", down, true);
      window.removeEventListener("keyup", up, true);
      window.removeEventListener("blur", blur);
    };
  }, [area]);

  return (
    <>
      <Button
        size="icon-sm"
        variant="ghost"
        title={`Dictate (${formatShortcut("Mod+E")})`}
        aria-label="Dictate"
        aria-pressed={phase !== "idle"}
        disabled={disabled || !ready || phase === "stopping"}
        onPointerDown={(e) => {
          if (e.button !== 0) return;
          e.preventDefault();
          e.currentTarget.setPointerCapture(e.pointerId);
          press("pointer");
        }}
        onPointerUp={() => release("pointer")}
        onPointerCancel={() => {
          pressed.current = null;
          void finish(true);
        }}
        onKeyDown={(e) => {
          if (e.key === " " || e.key === "Enter") {
            e.preventDefault();
            if (!e.repeat) press("pointer");
          }
        }}
        onKeyUp={(e) => {
          if (e.key === " " || e.key === "Enter") {
            e.preventDefault();
            release("pointer");
          }
        }}
        onClick={(e) => {
          if (e.detail === 0 && !pressed.current) {
            if (currentPhase.current === "idle") start();
            else void finish();
          }
        }}
      >
        <Mic className={phase === "listening" ? "text-brand" : undefined} />
      </Button>
      {presence.mounted && (
        <div className="absolute bottom-full left-1/2 z-20 mb-3 w-max max-w-full -translate-x-1/2 px-2">
          <div
            className="presence-pop bg-background flex items-center gap-3 rounded-full border px-3 py-2 text-ui shadow-lg"
            data-state={presence.state}
            role={error ? "alert" : "status"}
            aria-label="Dictation"
          >
            <Button
              size="icon-sm"
              variant="ghost"
              aria-label="Cancel dictation"
              onClick={() => {
                if (currentPhase.current === "idle") setError(null);
                else void finish(true);
              }}
            >
              <X />
            </Button>
            {error ? (
              <div className="max-w-sm">
                <p>{error}</p>
                {openSettings && (
                  <button className="text-brand hover:underline" onClick={() => openSettings({ tab: "voice" })}>
                    Open Voice settings
                  </button>
                )}
              </div>
            ) : (
              <>
                <div className="flex h-5 shrink-0 items-center gap-1" aria-hidden="true">
                  {Array.from({ length: 7 }, (_, i) => (
                    <span
                      key={i}
                      className="bg-brand size-1 rounded-full transition-transform duration-120 ease-std [transform:scaleY(var(--voice-level))] motion-reduce:transform-none motion-reduce:transition-none"
                      style={
                        {
                          "--voice-level": 1 + Math.min(1, rms * 12) * (4 - Math.abs(i - 3)),
                        } as CSSProperties
                      }
                    />
                  ))}
                </div>
                <span
                  className="text-muted-foreground min-w-0 max-w-72 truncate text-left [direction:rtl]"
                  title={partial}
                >
                  <bdi>
                    {phase === "starting"
                      ? "Opening microphone…"
                      : phase === "stopping"
                        ? "Transcribing…"
                        : partial || "Listening…"}
                  </bdi>
                </span>
                <span className="text-muted-foreground shrink-0 font-mono text-xs tabular-nums">
                  {Math.floor(elapsed / 60)}:{String(elapsed % 60).padStart(2, "0")}
                </span>
                <button
                  className="bg-brand text-brand-foreground grid size-7 shrink-0 place-items-center rounded-full enabled:hover:bg-brand/88 disabled:opacity-50"
                  aria-label="Insert dictation"
                  disabled={phase === "stopping"}
                  onClick={() => void finish()}
                >
                  <Check className="size-4" />
                </button>
              </>
            )}
          </div>
        </div>
      )}
    </>
  );
}
