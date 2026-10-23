// A sub-agent's model as its card names it: the short family-and-version
// name people use ("Sonnet 5.5"), not the id the agent reports.

const FAMILY = "(opus|sonnet|haiku)";
/** `claude-sonnet-5-5`, `claude-opus-4-1-20250805`, `claude-sonnet-4-20250514`. */
const CURRENT = new RegExp(`^claude-${FAMILY}-(\\d{1,2})(?:-(\\d{1,2}))?(?:-\\d{8})?$`);
/** `claude-3-5-sonnet-20241022`, `claude-3-opus-20240229`. */
const OLDER = new RegExp(`^claude-(\\d{1,2})(?:-(\\d{1,2}))?-${FAMILY}(?:-\\d{8})?$`);

const title = (family: string) => family[0]!.toUpperCase() + family.slice(1);
const version = (major: string, minor: string | undefined) => (minor ? `${major}.${minor}` : major);

/**
 * A model id as a short name: `claude-sonnet-5-5` → "Sonnet 5.5",
 * `claude-opus-4-1-20250805` → "Opus 4.1", `sonnet` → "Sonnet". Ids it does
 * not know pass through unchanged; no model is null.
 */
export function modelShortName(model: string | null | undefined): string | null {
  const id = model?.trim() ?? "";
  if (!id) return null;
  // A context-window tag ("[1m]") is not part of the name.
  const bare = id.replace(/\[[^\]]*\]$/, "").toLowerCase();
  const current = CURRENT.exec(bare);
  if (current) return `${title(current[1]!)} ${version(current[2]!, current[3])}`;
  const older = OLDER.exec(bare);
  if (older) return `${title(older[3]!)} ${version(older[1]!, older[2])}`;
  if (/^(opus|sonnet|haiku)$/.test(bare)) return title(bare);
  return id;
}
