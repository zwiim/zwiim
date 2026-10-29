// The Appearance tab's settings: how this window looks — its theme and
// colors, how big it draws, the fonts code and terminals use, and how a
// markdown file reads. Local to this computer.

import { Fragment, useEffect, useRef, useState, type ReactNode } from "react";
import { Check, ChevronDown, FileText, Minus, Monitor, Moon, Plus, Sun } from "lucide-react";
import { Markdown } from "@/components/Markdown";
import {
  fontChoices,
  fontInstalled,
  installedFonts,
  monoFonts,
  stepZoom,
  TERMINAL_FG,
} from "@/lib/appearance";
import type { FontFamily, NativeBridge } from "@/lib/bridge";
import { CODE_THEMES, HighlightedCode } from "@/lib/highlight";
import {
  cleanFontName,
  codeLigatures,
  codeTheme,
  colorVision,
  fonts,
  markdownStyle,
  rememberCodeLigatures,
  rememberCodeTheme,
  rememberColorVision,
  rememberFonts,
  rememberMarkdownStyle,
  rememberSoftDark,
  rememberTheme,
  rememberZoom,
  TERMINAL_SIZE_DEFAULT,
  TERMINAL_SIZE_MAX,
  TERMINAL_SIZE_MIN,
  theme,
  zoom,
  ZOOM_LEVELS,
  softDark,
  type CodeTheme,
  type ColorVision,
  type Fonts,
  type MarkdownStyle,
  type Theme,
} from "@/lib/prefs";
import { applyTheme } from "@/lib/theme";
import { usePref } from "@/lib/use-pref";
import { cn } from "@/lib/utils";
import { ControlSelect } from "@/panels/OptionsBar";
import type { SettingMeta } from "@/settings/registry";
import { SettingRow, ToggleRow } from "@/settings/rows";

const THEMES: { value: Theme; label: string; icon: typeof Sun }[] = [
  { value: "system", label: "System", icon: Monitor },
  { value: "light", label: "Light", icon: Sun },
  { value: "dark", label: "Dark", icon: Moon },
];

export function ThemeSetting({ setting }: { setting: SettingMeta }) {
  const [current, setCurrent] = useState(theme);
  return (
    <SettingRow setting={setting}>
      <div className="bg-muted flex gap-0.5 rounded-lg p-0.5" role="radiogroup" aria-label={setting.title}>
        {THEMES.map((t) => (
          <button
            key={t.value}
            className={cn(
              "grid size-7 place-items-center rounded-md transition-colors",
              current === t.value ? "bg-background text-foreground shadow-sm" : "text-muted-foreground hover:text-foreground",
            )}
            role="radio"
            aria-checked={current === t.value}
            aria-label={t.label}
            title={t.label}
            onClick={() => {
              rememberTheme(t.value);
              applyTheme(t.value);
              setCurrent(t.value);
            }}
          >
            <t.icon className="size-3.5" />
          </button>
        ))}
      </div>
    </SettingRow>
  );
}

/** A value stepped down and up, shown between its two buttons; the value itself resets it. */
function Stepper({
  label,
  value,
  canDown,
  canUp,
  onDown,
  onUp,
  onReset,
}: {
  label: string;
  value: string;
  canDown: boolean;
  canUp: boolean;
  onDown(): void;
  onUp(): void;
  onReset(): void;
}) {
  const step =
    "text-muted-foreground hover:text-foreground disabled:hover:text-muted-foreground grid size-7 place-items-center rounded-md transition-colors disabled:opacity-40";
  return (
    <div className="bg-muted flex items-center gap-0.5 rounded-lg p-0.5" role="group" aria-label={label}>
      <button className={step} aria-label={`Smaller ${label.toLowerCase()}`} disabled={!canDown} onClick={onDown}>
        <Minus className="size-3.5" />
      </button>
      <button
        className="text-ui hover:bg-background min-w-12 rounded-md px-1.5 py-1 font-mono tabular-nums transition-colors"
        title={`Reset ${label.toLowerCase()}`}
        onClick={onReset}
      >
        {value}
      </button>
      <button className={step} aria-label={`Larger ${label.toLowerCase()}`} disabled={!canUp} onClick={onUp}>
        <Plus className="size-3.5" />
      </button>
    </div>
  );
}

export function ZoomSetting({ setting }: { setting: SettingMeta }) {
  const current = usePref(zoom);
  return (
    <SettingRow setting={setting}>
      <Stepper
        label="Zoom"
        value={`${Math.round(current * 100)}%`}
        canDown={current > ZOOM_LEVELS[0]}
        canUp={current < ZOOM_LEVELS[ZOOM_LEVELS.length - 1]}
        onDown={() => rememberZoom(stepZoom(current, -1))}
        onUp={() => rememberZoom(stepZoom(current, 1))}
        onReset={() => rememberZoom(1)}
      />
    </SettingRow>
  );
}

/** One choice in a font picker: a family by name, empty for the default. */
interface FontRow {
  name: string;
  label: string;
  /** The first of the fonts that are not monospace. */
  other?: boolean;
}

/**
 * A font family picked from those installed here, or typed by name; empty for the default.
 * Its preview draws in the font the list is on, so each can be tried before it is picked.
 */
function FontSetting({
  setting,
  bridge,
  font,
  inherited,
  preview,
}: {
  setting: SettingMeta;
  bridge: NativeBridge;
  font: "code" | "terminal";
  /** The family the default draws in; empty for the app's own. */
  inherited: string;
  preview(fontFamily: string): ReactNode;
}) {
  const saved = usePref(() => fonts()[font]);
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [at, setAt] = useState(0);
  const [installed, setInstalled] = useState<FontFamily[] | null>(null);
  const root = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    let live = true;
    void installedFonts(bridge).then((list) => live && setInstalled(list));
    setQuery("");
    const close = (e: PointerEvent) => {
      if (!root.current?.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("pointerdown", close);
    return () => {
      live = false;
      document.removeEventListener("pointerdown", close);
    };
  }, [open, bridge]);

  const typed = cleanFontName(query);
  const { mono, other } = fontChoices(installed ?? [], query);
  const rows: FontRow[] = [
    ...(typed ? [] : [{ name: "", label: "Default" }]),
    ...mono.map((name) => ({ name, label: name })),
    ...other.map((name, i) => ({ name, label: name, other: i === 0 })),
  ];
  if (typed && !installed?.some((f) => f.name.toLowerCase() === typed.toLowerCase())) {
    rows.push({ name: typed, label: `Use “${typed}”` });
  }

  useEffect(() => {
    if (open) setAt(Math.max(0, rows.findIndex((r) => r.name === saved)));
    // Only when the list first shows or changes, not on every move.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, installed]);

  useEffect(() => {
    root.current?.querySelector(`[data-index="${at}"]`)?.scrollIntoView?.({ block: "nearest" });
  }, [at]);

  function pick(row: FontRow | undefined) {
    if (!row) return;
    if (row.name !== saved) rememberFonts({ [font]: row.name } as Partial<Fonts>);
    setOpen(false);
  }

  const fallback = inherited || "JetBrains Mono";
  const shown = open ? (rows[at]?.name ?? saved) : saved;
  const missing = saved !== "" && fontInstalled(saved) === false;
  return (
    <SettingRow setting={setting} below={preview(monoFonts(shown || inherited))}>
      <div ref={root} className="relative flex flex-col items-end gap-1">
        <button
          type="button"
          className="bg-input/30 hover:bg-muted flex w-48 items-center gap-2 rounded-md border px-2 py-1 text-left text-xs transition-colors"
          aria-haspopup="listbox"
          aria-expanded={open}
          aria-label={`${setting.title}: ${saved || "Default"}`}
          onClick={() => setOpen((o) => !o)}
        >
          {saved ? (
            <span className="min-w-0 flex-1 truncate" style={{ fontFamily: monoFonts(saved) }}>
              {saved}
            </span>
          ) : (
            <span className="min-w-0 flex-1 truncate">
              Default <span className="text-muted-foreground font-mono">{fallback}</span>
            </span>
          )}
          <ChevronDown className="text-muted-foreground size-3 shrink-0" />
        </button>
        {missing && <span className="text-muted-foreground text-xs">Not installed on this computer</span>}
        {open && (
          <div
            className="bg-popover text-foreground shadow-pop animate-pop-in absolute top-full right-0 z-20 mt-1 flex w-72 origin-top-right flex-col rounded-xl border"
            onKeyDown={(e) => {
              if (e.key === "Escape") {
                e.stopPropagation();
                setOpen(false);
              } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
                e.preventDefault();
                const n = rows.length;
                if (n) setAt((i) => (i + (e.key === "ArrowDown" ? 1 : n - 1)) % n);
              } else if (e.key === "Enter") {
                e.preventDefault();
                pick(rows[at]);
              }
            }}
          >
            <input
              className="placeholder:text-muted-foreground border-b bg-transparent px-3 py-2 text-sm outline-none"
              placeholder="Search fonts…"
              aria-label="Search fonts"
              autoFocus
              spellCheck={false}
              autoCapitalize="off"
              autoCorrect="off"
              value={query}
              onChange={(e) => {
                setQuery(e.target.value);
                setAt(0);
              }}
            />
            <ul className="max-h-64 overflow-y-auto p-1" role="listbox" aria-label={`${setting.title}s`}>
              {installed === null && <li className="text-muted-foreground px-2 py-2 text-xs">Looking for fonts…</li>}
              {rows.map((r, i) => (
                <Fragment key={`${r.label}:${r.name}`}>
                  {r.other && <li className="text-muted-foreground px-2 pt-2 pb-1 text-xs">Not monospace</li>}
                  <li
                    data-index={i}
                    role="option"
                    aria-selected={r.name === saved}
                    className={cn(
                      "flex cursor-pointer items-center gap-2 rounded-lg px-2 py-1.5 text-sm",
                      i === at && "bg-muted",
                    )}
                    onPointerEnter={() => setAt(i)}
                    onClick={() => pick(r)}
                  >
                    <Check className={cn("size-3.5 shrink-0", r.name !== saved && "invisible")} />
                    <span
                      className="truncate"
                      style={r.name && r.label === r.name ? { fontFamily: monoFonts(r.name) } : undefined}
                    >
                      {r.label}
                    </span>
                    {r.name === "" && (
                      <span className="text-muted-foreground ml-auto truncate font-mono text-xs">{fallback}</span>
                    )}
                  </li>
                </Fragment>
              ))}
            </ul>
          </div>
        )}
      </div>
    </SettingRow>
  );
}

/** One line of a diff, drawn as the transcript draws one: unchanged, removed or added. */
function DiffLine({ kind, children }: { kind: "" | "-" | "+"; children: string }) {
  return (
    <div className={cn("flex", kind === "+" && "bg-add", kind === "-" && "bg-del")}>
      <span
        className={cn(
          "w-5 shrink-0 text-center select-none",
          kind === "+" && "text-done",
          kind === "-" && "text-destructive",
        )}
      >
        {kind}
      </span>
      <span className="min-w-0 flex-1 truncate pr-3">{children}</span>
    </div>
  );
}

/** A file's change as the transcript shows one, with the glyphs fonts most often confuse. */
function CodePreview({ fontFamily }: { fontFamily: string }) {
  return (
    <div
      className="overflow-hidden rounded-lg border font-mono text-xs whitespace-pre"
      style={{ fontFamily }}
      aria-hidden
      data-testid="code-font-preview"
    >
      <div className="bg-card flex items-center gap-2 border-b px-3 py-2">
        <FileText className="text-muted-foreground size-3.5 shrink-0" />
        <span className="min-w-0 truncate">src/worktree.ts</span>
        <span className="ml-auto flex gap-1">
          <span className="text-done">+1</span>
          <span className="text-destructive">−1</span>
        </span>
      </div>
      <div className="bg-code py-1 leading-5">
        <DiffLine kind="">{"const port = 3000 + id; // 0O 1lI"}</DiffLine>
        <DiffLine kind="-">{'if (name != "") return `${prefix}/${name}`;'}</DiffLine>
        <DiffLine kind="+">{'if (name !== "") return `${prefix}/${name}`;'}</DiffLine>
      </div>
    </div>
  );
}

/** A command and what it printed, in the terminal's colours, font and size. */
function TerminalPreview({ fontFamily }: { fontFamily: string }) {
  const fontSize = usePref(() => fonts().terminalSize);
  return (
    <div
      className="bg-terminal overflow-hidden rounded-lg px-3 py-2 whitespace-pre"
      style={{ fontFamily, fontSize, color: TERMINAL_FG }}
      aria-hidden
      data-testid="terminal-font-preview"
    >
      <div>
        <span className="opacity-60">~/zwiim main</span> ❯ cargo test
      </div>
      <div>running 42 tests</div>
      <div>test result: ok. 42 passed; 0 failed</div>
    </div>
  );
}

export function CodeFontSetting({ setting, bridge }: { setting: SettingMeta; bridge: NativeBridge }) {
  return (
    <FontSetting
      setting={setting}
      bridge={bridge}
      font="code"
      inherited=""
      preview={(f) => <CodePreview fontFamily={f} />}
    />
  );
}

export function TerminalFontSetting({ setting, bridge }: { setting: SettingMeta; bridge: NativeBridge }) {
  const code = usePref(() => fonts().code);
  return (
    <FontSetting
      setting={setting}
      bridge={bridge}
      font="terminal"
      inherited={code}
      preview={(f) => <TerminalPreview fontFamily={f} />}
    />
  );
}

export function TerminalSizeSetting({ setting }: { setting: SettingMeta }) {
  const size = usePref(() => fonts().terminalSize);
  return (
    <SettingRow setting={setting}>
      <Stepper
        label="Terminal text size"
        value={`${size}px`}
        canDown={size > TERMINAL_SIZE_MIN}
        canUp={size < TERMINAL_SIZE_MAX}
        onDown={() => rememberFonts({ terminalSize: size - 1 })}
        onUp={() => rememberFonts({ terminalSize: size + 1 })}
        onReset={() => rememberFonts({ terminalSize: TERMINAL_SIZE_DEFAULT })}
      />
    </SettingRow>
  );
}

export function SoftDarkToggle({ setting }: { setting: SettingMeta }) {
  const on = usePref(softDark);
  return <ToggleRow setting={setting} testId="soft-dark" checked={on} onChange={rememberSoftDark} />;
}

const COLOR_VISIONS: { value: ColorVision; label: string }[] = [
  { value: "default", label: "Default" },
  { value: "red_green", label: "Red-green" },
  { value: "blue_yellow", label: "Blue-yellow" },
];

export function ColorVisionSetting({ setting }: { setting: SettingMeta }) {
  const current = usePref(colorVision);
  return (
    <SettingRow setting={setting}>
      <ControlSelect
        label={setting.title}
        value={current}
        onChange={(v) => rememberColorVision(v as ColorVision)}
        options={COLOR_VISIONS}
      />
    </SettingRow>
  );
}

export function CodeLigaturesToggle({ setting }: { setting: SettingMeta }) {
  const on = usePref(codeLigatures);
  return <ToggleRow setting={setting} testId="code-ligatures" checked={on} onChange={rememberCodeLigatures} />;
}

const CODE_THEME_OPTIONS = CODE_THEMES.map((t) => ({ value: t.id, label: t.label }));

const CODE_THEME_SAMPLE = `// Branch names for new worktrees
export function branchFor(task: Task, prefix = "zwiim") {
  const slug = task.title.replace(/\\W+/g, "-");
  return \`\${prefix}/\${slug}-\${task.id % 1000}\`;
}`;

export function CodeThemeSetting({ setting }: { setting: SettingMeta }) {
  const current = usePref(codeTheme);
  return (
    <SettingRow
      setting={setting}
      below={
        <pre
          className="bg-code overflow-hidden rounded-lg border px-4 py-3 font-mono text-xs leading-5"
          aria-hidden
          data-testid="code-theme-preview"
        >
          <code>
            <HighlightedCode code={CODE_THEME_SAMPLE} lang="typescript" />
          </code>
        </pre>
      }
    >
      <ControlSelect
        label={setting.title}
        value={current}
        onChange={(v) => rememberCodeTheme(v as CodeTheme)}
        options={CODE_THEME_OPTIONS}
      />
    </SettingRow>
  );
}

const MARKDOWN_STYLES: { value: MarkdownStyle; label: string }[] = [
  { value: "default", label: "Default" },
  { value: "book", label: "Book" },
];

const MARKDOWN_SAMPLE = `### Release notes

The host now **keeps** each terminal's output across restarts, so a long build is never lost.

> Read the changelog for everything else.`;

export function MarkdownStyleSetting({ setting }: { setting: SettingMeta }) {
  const current = usePref(markdownStyle);
  return (
    <SettingRow
      setting={setting}
      below={
        <div className="bg-card overflow-hidden rounded-lg border" aria-hidden data-testid="markdown-style-preview">
          <Markdown
            text={MARKDOWN_SAMPLE}
            className={current === "book" ? "markdown-book px-6 py-4" : "px-4 py-3"}
          />
        </div>
      }
    >
      <ControlSelect
        label={setting.title}
        value={current}
        onChange={(v) => rememberMarkdownStyle(v as MarkdownStyle)}
        options={MARKDOWN_STYLES}
      />
    </SettingRow>
  );
}
