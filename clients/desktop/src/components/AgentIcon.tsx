import type { ReactNode } from "react";

export interface AgentBrand {
  short: string;
  tile: string;
  ink: string;
  dot: string;
}

const BRANDS: Record<string, AgentBrand> = {
  claude_code: { short: "Claude", tile: "#D97757", ink: "#FFFFFF", dot: "#D97757" },
  codex: { short: "Codex", tile: "#454B52", ink: "#FFFFFF", dot: "#B8BEC6" },
  opencode: { short: "OpenCode", tile: "#F2F3F5", ink: "#111111", dot: "#F2F3F5" },
  gemini: { short: "Gemini", tile: "#4E86F6", ink: "#FFFFFF", dot: "#4E86F6" },
};

export function agentBrand(agent: string, name: string): AgentBrand {
  return (
    BRANDS[agent] ?? {
      short: name,
      tile: "var(--secondary)",
      ink: "var(--foreground)",
      dot: "var(--muted-foreground)",
    }
  );
}

const GLYPHS: Record<string, { strokeWidth?: number; fill?: boolean; body: ReactNode }> = {
  claude_code: {
    strokeWidth: 2.4,
    body: <path d="M12 3v18M3 12h18M5.6 5.6l12.8 12.8M18.4 5.6 5.6 18.4" />,
  },
  codex: {
    body: (
      <>
        <circle cx="12" cy="12" r="7.5" />
        <path d="M8.5 10.5 11 12.5l-2.5 2M13 15h3" />
      </>
    ),
  },
  opencode: { body: <path d="M9 6H6v12h3M15 6h3v12h-3" /> },
  gemini: {
    fill: true,
    body: <path d="M12 2.5c.8 5 4.5 8.7 9.5 9.5-5 .8-8.7 4.5-9.5 9.5-.8-5-4.5-8.7-9.5-9.5 5-.8 8.7-4.5 9.5-9.5z" />,
  },
};

/** The agent's brand tile: its glyph, or the first letter of its name when it has none. */
export function AgentIcon({ agent, name, size = 26 }: { agent: string; name: string; size?: 18 | 26 }) {
  const brand = agentBrand(agent, name);
  const glyph = GLYPHS[agent];
  const px = size === 26 ? 16 : 12;
  return (
    <span
      aria-hidden="true"
      className="inline-flex shrink-0 items-center justify-center font-semibold"
      style={{
        width: size,
        height: size,
        borderRadius: size === 26 ? 7 : 5,
        background: brand.tile,
        color: brand.ink,
        boxShadow: "inset 0 0 0 1px rgba(255,255,255,.08)",
        fontSize: size === 26 ? 13 : 10,
      }}
    >
      {glyph ? (
        <svg
          width={px}
          height={px}
          viewBox="0 0 24 24"
          fill={glyph.fill ? "currentColor" : "none"}
          stroke={glyph.fill ? "none" : "currentColor"}
          strokeWidth={glyph.strokeWidth ?? 2}
          strokeLinecap="round"
          strokeLinejoin="round"
        >
          {glyph.body}
        </svg>
      ) : (
        name.charAt(0).toUpperCase()
      )}
    </span>
  );
}

/** A small dot in the agent's color, for places too tight for the tile. */
export function AgentDot({ agent, name }: { agent: string; name: string }) {
  return (
    <span
      aria-hidden="true"
      className="inline-block size-[7px] shrink-0 rounded-full"
      style={{ background: agentBrand(agent, name).dot, boxShadow: "0 0 0 1px rgba(255,255,255,.28)" }}
    />
  );
}
