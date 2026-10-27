// When a clock starts for what the Host does not date: when this client
// first saw it.

/** When `id` was first seen, by `seen`'s record; now when it is new. Keeps the last 64. */
export function firstSeen(seen: Map<string, number>, id: string): number {
  let at = seen.get(id);
  if (at === undefined) {
    at = Date.now();
    seen.set(id, at);
    if (seen.size > 64) seen.delete(seen.keys().next().value!);
  }
  return at;
}

/** When each sub-agent launch was first seen. */
const launchStarts = new Map<string, number>();

/** When the launch `id` was first seen, for a Host that does not say when its sub-agent started. */
export function launchFirstSeen(id: string): number {
  return firstSeen(launchStarts, id);
}
