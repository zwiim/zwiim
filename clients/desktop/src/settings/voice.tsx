import { useEffect, useState } from "react";
import { Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import type { VoiceBridge, VoiceEvents, VoiceModel } from "@/lib/bridge";
import { errorMessage } from "@/lib/host-context";
import { formatShortcut } from "@/lib/platform";
import { rememberVoice, voice, type VoicePrefs } from "@/lib/prefs";
import { usePref } from "@/lib/use-pref";
import { cn } from "@/lib/utils";
import { ControlSelect } from "@/panels/OptionsBar";
import type { SettingDef, SettingMeta } from "@/settings/registry";
import { SettingRow, ToggleRow } from "@/settings/rows";

const MODES = {
  auto: "Tap to start or stop. Hold for more than 400 ms to talk until you release.",
  hold: "Hold the shortcut or microphone button while you talk. Release to insert the words.",
  toggle: "Press once to start, then again to insert the words.",
};
const LANGUAGES = [
  ["auto", "Auto-detect"],
  ["en", "English"],
  ["es", "Spanish"],
  ["fr", "French"],
  ["de", "German"],
  ["it", "Italian"],
  ["pt", "Portuguese"],
  ["nl", "Dutch"],
  ["hi", "Hindi"],
  ["zh", "Chinese"],
  ["ja", "Japanese"],
  ["ko", "Korean"],
].map(([value, label]) => ({ value: value!, label: label! }));

export const VOICE_SETTINGS: SettingDef[] = [
  {
    id: "voice-enabled",
    title: "Dictation",
    description: `Talk into the composer with ${formatShortcut("Mod+E")}. Speech is recognized on this computer.`,
  },
  { id: "voice-mode", title: "Mode" },
  {
    id: "voice-model",
    title: "Speech model",
    description: "Download a model once, then dictate offline. Audio and text stay on this computer.",
  },
  { id: "voice-language", title: "Language" },
  { id: "voice-input", title: "Input" },
  { id: "voice-silence", title: "Stop after silence" },
  {
    id: "voice-fillers",
    title: "Remove filler words",
    description: "Leave out um, uh, erm and hmm.",
  },
  {
    id: "voice-vocabulary",
    title: "Vocabulary",
    description: "Use Heard as for words that are often misheard, or leave it blank to fix the casing.",
  },
  {
    id: "voice-gpu",
    title: "Use GPU",
    description: "Turn off if dictation freezes your computer.",
  },
  { id: "voice-loaded", title: "Keep model loaded" },
].map((s) => ({
  ...s,
  tab: "voice",
  keywords: ["dictation", "speech", "microphone", "talk", "whisper", "parakeet", "transcribe"],
  available: (ctx) => !!ctx.bridge.voice,
  render: (ctx, setting) => ctx.bridge.voice && <VoiceSetting bridge={ctx.bridge.voice} setting={setting} />,
}));

function Heading({ children }: { children: React.ReactNode }) {
  return <h3 className="text-muted-foreground pt-3 text-2xs font-semibold uppercase tracking-wide">{children}</h3>;
}

export function VoiceSetting({ bridge, setting }: { bridge: VoiceBridge; setting: SettingMeta }) {
  usePref(() => JSON.stringify(voice()));
  const current = voice();
  const [inputs, setInputs] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    if (setting.id !== "voice-input") return;
    let live = true;
    bridge
      .inputs()
      .then((names) => {
        if (live) setInputs(names);
      })
      .catch((e) => {
        if (live) setError(errorMessage(e));
      });
    return () => {
      live = false;
    };
  }, [bridge, setting.id]);
  const select = (
    key: "input" | "language" | "silenceSecs" | "keepLoadedSecs",
    options: { value: string; label: string }[],
    description?: string,
    disabled?: boolean,
  ) => (
    <SettingRow
      setting={{ ...setting, description: description ?? setting.description }}
      below={
        error && (
          <p role="alert" className="text-destructive">
            {error}
          </p>
        )
      }
    >
      <ControlSelect
        label={setting.title}
        value={String(current[key])}
        options={options}
        disabled={disabled}
        onChange={(value) => {
          const next = key === "silenceSecs" || key === "keepLoadedSecs" ? Number(value) : value;
          rememberVoice({ [key]: next });
          if (key === "keepLoadedSecs") {
            setError(null);
            void bridge.unloadAfter(Number(value)).catch((e) => setError(errorMessage(e)));
          }
        }}
      />
    </SettingRow>
  );
  switch (setting.id) {
    case "voice-enabled":
      return (
        <ToggleRow setting={setting} checked={current.enabled} onChange={(enabled) => rememberVoice({ enabled })} />
      );
    case "voice-mode":
      return (
        <SettingRow setting={{ ...setting, description: MODES[current.mode] }}>
          <div className="bg-muted flex gap-0.5 rounded-lg p-0.5" role="radiogroup" aria-label="Mode">
            {(["auto", "hold", "toggle"] as const).map((mode) => (
              <button
                key={mode}
                role="radio"
                aria-checked={current.mode === mode}
                className={cn(
                  "text-ui rounded-md px-2.5 py-1 transition-colors",
                  current.mode === mode
                    ? "bg-background text-foreground shadow-sm"
                    : "text-muted-foreground hover:text-foreground",
                )}
                onClick={() => rememberVoice({ mode })}
              >
                {mode[0]!.toUpperCase() + mode.slice(1)}
              </button>
            ))}
          </div>
        </SettingRow>
      );
    case "voice-model":
      return <VoiceModels bridge={bridge} setting={setting} selected={current.model} />;
    case "voice-language":
      return select(
        "language",
        LANGUAGES,
        current.model.startsWith("parakeet")
          ? `Parakeet TDT ${current.model.endsWith("v2") ? "v2 recognizes English only" : "v3 detects it by itself"}.`
          : undefined,
        current.model.startsWith("parakeet"),
      );
    case "voice-input":
      return (
        <>
          <Heading>Microphone</Heading>
          {select("input", [
            { value: "", label: "System default" },
            ...inputs.map((name) => ({ value: name, label: name })),
          ])}
        </>
      );
    case "voice-silence":
      return select("silenceSecs", [
        { value: "0", label: "Never" },
        { value: "30", label: "30 seconds" },
        { value: "60", label: "1 minute" },
        { value: "120", label: "2 minutes" },
      ]);
    case "voice-fillers":
      return (
        <>
          <Heading>Words</Heading>
          <ToggleRow
            setting={setting}
            checked={current.removeFillers}
            onChange={(removeFillers) => rememberVoice({ removeFillers })}
          />
        </>
      );
    case "voice-vocabulary":
      return <Vocabulary setting={setting} vocabulary={current.vocabulary} />;
    case "voice-gpu":
      return (
        <>
          <Heading>Advanced</Heading>
          <ToggleRow setting={setting} checked={current.gpu} onChange={(gpu) => rememberVoice({ gpu })} />
        </>
      );
    case "voice-loaded":
      return select("keepLoadedSecs", [
        { value: "0", label: "Unload right away" },
        { value: "300", label: "5 minutes" },
        { value: "900", label: "15 minutes" },
        { value: "-1", label: "Always" },
      ]);
    default:
      return null;
  }
}

function VoiceModels({ bridge, setting, selected }: { bridge: VoiceBridge; setting: SettingMeta; selected: string }) {
  const [models, setModels] = useState<VoiceModel[] | null>(null);
  const [progress, setProgress] = useState<Record<string, VoiceEvents["download"]>>({});
  const [pending, setPending] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    let unlisten: (() => void) | undefined;
    const refresh = () =>
      bridge.models().then((m) => {
        if (live) setModels(m);
      });
    void bridge
      .listen("download", (p) => {
        if (!live) return;
        setProgress((old) => ({ ...old, [p.id]: p }));
        if (p.error) setError(p.error);
        // Either way the shell's download ended; read which models are installed and still downloading.
        if (p.done || p.error)
          void refresh().catch((e) => {
            if (live) setError(errorMessage(e));
          });
      })
      .then((off) => {
        if (live) unlisten = off;
        else off();
      })
      .catch((e) => {
        if (live) setError(errorMessage(e));
      });
    void refresh().catch((e) => {
      if (live) setError(errorMessage(e));
    });
    return () => {
      live = false;
      unlisten?.();
    };
  }, [bridge]);
  async function change(id: string, download: boolean) {
    setError(null);
    setPending((old) => [...old, id]);
    setProgress((old) => ({ ...old, [id]: { id, downloaded: 0 } }));
    try {
      if (download) await bridge.download(id);
      else await bridge.delete(id);
      setModels(await bridge.models());
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setPending((old) => old.filter((p) => p !== id));
    }
  }
  return (
    <SettingRow
      setting={setting}
      below={
        <>
          {error && (
            <p role="alert" className="text-destructive">
              {error}
            </p>
          )}
          {models === null ? (
            <div className="bg-muted h-24 rounded-md" aria-label="Loading speech models" />
          ) : (
            <table className="w-full text-left text-xs" aria-label="Speech models">
              <thead className="text-muted-foreground">
                <tr>
                  <th className="pb-2 font-medium">Model</th>
                  <th className="px-2 pb-2 font-medium">Quality</th>
                  <th className="px-2 pb-2 font-medium">Size</th>
                  <th className="pb-2">
                    <span className="sr-only">Action</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {models.map((m) => {
                  const p = progress[m.id];
                  const downloading =
                    !m.installed &&
                    (m.downloading || pending.includes(m.id) || (!!p && !p.done && !p.error && (p.downloaded ?? 0) > 0));
                  return (
                    <tr key={m.id} className="border-t">
                      <td className="py-3">
                        <span className="text-ui font-medium">{m.label}</span>
                        {m.recommended && (
                          <span className="bg-muted ml-2 rounded px-1.5 py-0.5 text-2xs">Recommended</span>
                        )}
                        <p className="text-muted-foreground mt-1">{m.description}</p>
                      </td>
                      <td className="text-muted-foreground px-2">{m.quality}</td>
                      <td className="text-muted-foreground whitespace-nowrap px-2 font-mono">
                        {Math.round(m.size_mb)} MB
                      </td>
                      <td className="w-28 py-3 text-right">
                        {m.installed ? (
                          <div className="flex items-center justify-end gap-1">
                            {selected === m.id ? (
                              <span className="text-muted-foreground">In use</span>
                            ) : (
                              <Button size="xs" variant="ghost" onClick={() => rememberVoice({ model: m.id })}>
                                Use
                              </Button>
                            )}
                            <Button
                              size="icon-sm"
                              variant="ghost"
                              title={`Delete ${m.label}`}
                              aria-label={`Delete ${m.label}`}
                              disabled={pending.includes(m.id)}
                              onClick={() => void change(m.id, false)}
                            >
                              <Trash2 />
                            </Button>
                          </div>
                        ) : downloading ? (
                          <div className="flex flex-col gap-1">
                            <span className="text-muted-foreground font-mono text-2xs">
                              {Math.round((p?.downloaded ?? 0) / 1e6)} /{" "}
                              {Math.round((p?.total ?? m.size_mb * 1e6) / 1e6)} MB
                            </span>
                            <progress
                              aria-label={`Downloading ${m.label}`}
                              className="h-1 w-full accent-brand"
                              value={p?.downloaded ?? 0}
                              max={p?.total || m.size_mb * 1e6}
                            />
                          </div>
                        ) : (
                          <Button size="xs" variant="outline" onClick={() => void change(m.id, true)}>
                            Download
                          </Button>
                        )}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          )}
        </>
      }
    >
      {null}
    </SettingRow>
  );
}

function Vocabulary({ setting, vocabulary }: { setting: SettingMeta; vocabulary: VoicePrefs["vocabulary"] }) {
  const [word, setWord] = useState("");
  const [heardAs, setHeardAs] = useState("");
  return (
    <SettingRow
      setting={setting}
      below={
        <>
          <form
            className="flex gap-2"
            onSubmit={(e) => {
              e.preventDefault();
              if (!word.trim()) return;
              rememberVoice({
                vocabulary: [...vocabulary, { word: word.trim(), heardAs: heardAs.trim() }],
              });
              setWord("");
              setHeardAs("");
            }}
          >
            <input
              className="bg-muted min-w-0 flex-1 rounded-md border px-2 py-1.5"
              aria-label="Word"
              placeholder="Word"
              value={word}
              onChange={(e) => setWord(e.target.value)}
            />
            <input
              className="bg-muted min-w-0 flex-1 rounded-md border px-2 py-1.5"
              aria-label="Heard as (optional)"
              placeholder="Heard as (optional)"
              value={heardAs}
              onChange={(e) => setHeardAs(e.target.value)}
            />
            <Button type="submit" size="xs" variant="outline" disabled={!word.trim()}>
              Add
            </Button>
          </form>
          {vocabulary.length ? (
            <ul>
              {vocabulary.map((v, index) => (
                <li key={index} className="flex items-center gap-2 border-b py-1">
                  <span className="flex-1">
                    {v.word}
                    {v.heardAs && <span className="text-muted-foreground"> · {v.heardAs}</span>}
                  </span>
                  <Button
                    size="icon-sm"
                    variant="ghost"
                    aria-label={`Remove ${v.word}`}
                    onClick={() =>
                      rememberVoice({
                        vocabulary: vocabulary.filter((_, i) => i !== index),
                      })
                    }
                  >
                    <Trash2 />
                  </Button>
                </li>
              ))}
            </ul>
          ) : (
            <p className="text-muted-foreground">No words added yet.</p>
          )}
        </>
      }
    >
      {null}
    </SettingRow>
  );
}
