// A file change read for display. The parsers live in the SDK, shared with
// the mobile app; this module keeps the app's import path.

export {
  basename,
  editChanges,
  parseCodexPatch,
  parseUnifiedDiff,
  textDiff,
  toolChanges,
  type DiffLine,
  type FileDiff,
} from "@zwiim/sdk";
