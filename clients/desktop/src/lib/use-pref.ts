import { useSyncExternalStore } from "react";
import { onPrefsChange } from "@/lib/prefs";

/**
 * A preference that re-renders its reader when it is changed anywhere in this
 * window. `read` must return a primitive or otherwise stable value.
 */
export function usePref<T>(read: () => T): T {
  return useSyncExternalStore(onPrefsChange, read);
}
