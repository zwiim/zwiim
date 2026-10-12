// The app's control icons: tabs, back, close, disclosure, search, new, send, and
// the session's mode, options and context chips. Lucide
// line icons at one stroke weight. Glyphs that are machine text (✓ ✕ ⌂ ☁ ⎇ in
// rows, pills and empty states) stay mono text, per DESIGN.md.

import type { LucideIcon } from "lucide-react-native";

export {
  ArrowUp,
  Bot,
  Check,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  ChevronUp,
  ChevronsUpDown,
  CircleAlert,
  CircleCheckBig,
  Copy,
  Ellipsis,
  Eye,
  FileText,
  Folder,
  Gauge,
  GitBranchPlus,
  GitMerge,
  GitPullRequest,
  Image,
  Laptop,
  ListChecks,
  Lock,
  MessagesSquare,
  Paperclip,
  Plus,
  RotateCw,
  ScanLine,
  Search,
  Shield,
  ShieldOff,
  X,
} from "lucide-react-native";

/** An icon at the app's stroke weight; the stroke stays 1.75pt at any size. */
export function Icon({ icon: Glyph, color, size = 20 }: { icon: LucideIcon; color: string; size?: number }) {
  return <Glyph color={color} size={size} strokeWidth={1.75} absoluteStrokeWidth />;
}
