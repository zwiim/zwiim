// Opening settings at a place: a tab, and optionally one setting in it, which
// the dialog scrolls to and marks. Anything below the workbench asks for it
// through the context rather than by threading a callback down.

import { createContext, useContext } from "react";
import type { SettingsTab } from "@/settings/registry";

/** Where settings open. Each request is a new object, so asking twice marks the setting twice. */
export interface SettingsTarget {
  tab: SettingsTab;
  settingId?: string;
  /** The workspace the repository tab shows, in place of the window's. */
  workspaceId?: string;
}

export type OpenSettings = (target: SettingsTarget) => void;

export const OpenSettingsContext = createContext<OpenSettings | null>(null);

/** Opens settings at a place; null where no settings dialog is offered. */
export function useOpenSettings(): OpenSettings | null {
  return useContext(OpenSettingsContext);
}
