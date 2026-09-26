// Following every live session of the host. The host announces a request
// (permission, question, plan approval) only on the session's own event
// stream, so knowing what waits for the user means streaming each session
// whose agent is running. A session the host no longer runs is let go,
// unless it is on screen: a shown session also has its transcript loaded,
// and keeps both until it is hidden.
//
// The SDK does the streaming: `HostClient.attachSession` resumes from its
// cursor after a reconnect, and the `HostController` reduces every frame
// into the store by session id, so a replay never duplicates a session, a
// request or a message.

import type { HostController, HostState, HostStore } from "@zwiim/sdk";

/** The ids of the sessions whose agent is running, as the host lists them. */
export function liveSessionIds(state: Pick<HostState, "sessionsByWorkspace">): Set<string> {
  const live = new Set<string>();
  for (const list of Object.values(state.sessionsByWorkspace)) {
    for (const s of list) if (s.live) live.add(s.session_id);
  }
  return live;
}

export class SessionFollower {
  readonly #controller: HostController;
  readonly #store: HostStore;
  /** Sessions streamed because their agent runs. */
  readonly #followed = new Set<string>();
  /** When this phone first saw each pending request (ms since the epoch). */
  readonly #seen: Map<string, number>;
  /** Sessions on screen. */
  readonly #shown: Set<string>;
  readonly #off: () => void;
  #lists: HostState["sessionsByWorkspace"] | null = null;
  #views: HostState["sessions"] | null = null;

  /**
   * `seen` and `shown` may outlive the follower: a new connection to the same
   * host keeps them, and shows the same sessions again.
   */
  constructor(
    controller: HostController,
    store: HostStore,
    seen = new Map<string, number>(),
    shown = new Set<string>(),
    readonly now: () => number = Date.now,
  ) {
    this.#controller = controller;
    this.#store = store;
    this.#seen = seen;
    this.#shown = shown;
    for (const id of shown) controller.openSession(id);
    this.#off = store.subscribe(() => this.#sync());
    this.#sync();
  }

  /** When this phone first saw a request still pending; null for any other. */
  seenAt(request_id: string): number | null {
    return this.#seen.get(request_id) ?? null;
  }

  /** A session is on screen: stream it and load its transcript. */
  show(session_id: string): void {
    if (this.#shown.has(session_id)) return;
    this.#shown.add(session_id);
    this.#controller.openSession(session_id);
  }

  /** Off screen again: a running session keeps streaming for its requests. */
  hide(session_id: string): void {
    if (!this.#shown.delete(session_id)) return;
    if (!this.#followed.has(session_id)) this.#controller.closeSession(session_id);
  }

  dispose(): void {
    this.#off();
    for (const id of new Set([...this.#followed, ...this.#shown])) this.#controller.client.detachSession(id);
    this.#followed.clear();
  }

  #sync(): void {
    const state = this.#store.getState();
    if (state.sessionsByWorkspace !== this.#lists) {
      this.#lists = state.sessionsByWorkspace;
      const live = liveSessionIds(state);
      for (const id of live) {
        if (this.#followed.has(id)) continue;
        this.#followed.add(id);
        // A shown session whose agent had stopped streams again.
        this.#controller.client.attachSession(id);
      }
      for (const id of [...this.#followed]) {
        if (live.has(id)) continue;
        this.#followed.delete(id);
        if (!this.#shown.has(id)) this.#controller.closeSession(id);
      }
    }
    // Read again: letting a session go above changed the views.
    const { sessions } = this.#store.getState();
    if (sessions !== this.#views) {
      this.#views = sessions;
      const pending = new Set<string>();
      for (const view of Object.values(sessions)) {
        const id = view.pendingRequest?.request_id;
        if (!id) continue;
        pending.add(id);
        if (!this.#seen.has(id)) this.#seen.set(id, this.now());
      }
      for (const id of [...this.#seen.keys()]) if (!pending.has(id)) this.#seen.delete(id);
    }
  }
}
