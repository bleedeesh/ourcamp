/**
 * Minimal observable store.
 *
 * The app state is small and owned by a few controllers, so a dedicated state library
 * would add API surface without adding capability. React subscribes through
 * `useSyncExternalStore` (see `use-store.ts`), which gives tearing-free reads in
 * concurrent rendering.
 */

export type Listener = () => void;

export interface Store<S> {
  /** Current state. Treat it as immutable. */
  get(): S;
  /** Replaces the state with the result of `update`; listeners run only if it changed. */
  update(update: (state: S) => S): void;
  /** Merges a partial state (shallow); listeners run only if a value changed. */
  patch(partial: Partial<S>): void;
  /** Registers a listener and returns the function that removes it. */
  subscribe(listener: Listener): () => void;
}

export function createStore<S extends object>(initial: S): Store<S> {
  let state = initial;
  const listeners = new Set<Listener>();

  const commit = (next: S): void => {
    if (Object.is(next, state)) return;
    state = next;
    // Copy so a listener that unsubscribes (or subscribes) mid-notification is safe.
    for (const listener of [...listeners]) listener();
  };

  return {
    get: () => state,
    update: (update) => commit(update(state)),
    patch: (partial) => {
      const keys = Object.keys(partial) as (keyof S)[];
      if (keys.every((k) => Object.is(state[k], partial[k]))) return;
      commit({ ...state, ...partial });
    },
    subscribe: (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}
