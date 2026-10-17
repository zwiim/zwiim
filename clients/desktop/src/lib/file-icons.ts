// File-type icons from Material Icon Theme (MIT; see third_party/material-icon-theme/PROVENANCE.md):
// the pack's own SVGs, looked up with the pack's own name and extension tables.
import manifest from "material-icon-theme/dist/material-icons.json";

const urls = import.meta.glob<string>("../../../../node_modules/material-icon-theme/icons/*.svg", {
  eager: true,
  query: "?url",
  import: "default",
});
const ICONS = new Map<string, string>();
for (const [path, url] of Object.entries(urls)) ICONS.set(path.slice(path.lastIndexOf("/") + 1, -4), url);

type Table = Record<string, string>;
const lower = (table: Table | undefined): Table =>
  Object.fromEntries(Object.entries(table ?? {}).map(([k, v]) => [k.toLowerCase(), v]));

const DARK = {
  fileNames: lower(manifest.fileNames),
  fileExtensions: lower(manifest.fileExtensions),
  folderNames: lower(manifest.folderNames),
  folderNamesExpanded: lower(manifest.folderNamesExpanded),
};
const LIGHT = {
  fileNames: lower(manifest.light.fileNames),
  fileExtensions: lower(manifest.light.fileExtensions),
  folderNames: lower(manifest.light.folderNames),
  folderNamesExpanded: lower(manifest.light.folderNamesExpanded),
};

const isLight = () => typeof document !== "undefined" && !document.documentElement.classList.contains("dark");
const url = (id: string | undefined) => (id ? ICONS.get(id) : undefined);

/** The icon id for a file: its exact name, then its longest known extension (`test.tsx` before `tsx`). */
export function fileIconId(name: string, light = isLight()): string {
  const base = name.slice(name.lastIndexOf("/") + 1).toLowerCase();
  const pick = (key: keyof typeof DARK, k: string) => (light && LIGHT[key][k]) || DARK[key][k];
  const named = pick("fileNames", base);
  if (named && ICONS.has(named)) return named;
  for (let dot = base.indexOf("."); dot >= 0; dot = base.indexOf(".", dot + 1)) {
    const id = pick("fileExtensions", base.slice(dot + 1));
    if (id && ICONS.has(id)) return id;
  }
  return manifest.file;
}

/** The icon id for a folder, open or closed, by its name (`src`, `components`, `.github`). */
export function folderIconId(name: string, open: boolean, light = isLight()): string {
  const base = name.toLowerCase();
  const key = open ? "folderNamesExpanded" : "folderNames";
  const id = (light && LIGHT[key][base]) || DARK[key][base];
  return id && ICONS.has(id) ? id : open ? manifest.folderExpanded : manifest.folder;
}

/** The served URL of an icon id; an unknown id draws the generic file. */
export const iconUrl = (id: string): string => url(id) ?? url(manifest.file)!;
export const fileIconUrl = (name: string): string => iconUrl(fileIconId(name));
