// The app's control icons: tabs, back, close, disclosure and send. Lucide
// line icons at one stroke weight. Glyphs that are machine text (✓ ✕ ⌂ ☁ ⎇ in
// rows, pills and empty states) stay mono text, per DESIGN.md.

import type { LucideIcon } from "lucide-react-native";

export {
  ArrowUp,
  ChevronDown,
  ChevronLeft,
  Folder,
  Laptop,
  MessagesSquare,
  X,
} from "lucide-react-native";

/** An icon at the app's stroke weight; the stroke stays 1.75pt at any size. */
export function Icon({ icon: Glyph, color, size = 20 }: { icon: LucideIcon; color: string; size?: number }) {
  return <Glyph color={color} size={size} strokeWidth={1.75} absoluteStrokeWidth />;
}
