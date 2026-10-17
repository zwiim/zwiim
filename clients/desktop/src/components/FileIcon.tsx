import { memo } from "react";
import { fileIconId, folderIconId, iconUrl } from "@/lib/file-icons";
import { cn } from "@/lib/utils";

type IconProps = { className?: string };
type Icon = (props: IconProps) => React.ReactElement;

// One component per icon id, so a row keeps its <img> across renders.
const components = new Map<string, Icon>();
function iconComponent(id: string): Icon {
  let component = components.get(id);
  if (!component) {
    component = memo(({ className }: IconProps) => (
      <img src={iconUrl(id)} alt="" aria-hidden draggable={false} className={cn("size-4 shrink-0", className)} />
    )) as unknown as Icon;
    components.set(id, component);
  }
  return component;
}

/** A file's type at a glance: the brand logo an editor's tree shows (Rust, React, HTML…). */
export function fileIcon(name: string): { icon: Icon; color: string } {
  return { icon: iconComponent(fileIconId(name)), color: "" };
}

/** A folder's icon, open or closed, by its name (`src`, `components`, `.github`). */
export function FolderIcon({ name, open, className }: { name: string; open: boolean } & IconProps) {
  const Icon = iconComponent(folderIconId(name, open));
  return <Icon className={className} />;
}
