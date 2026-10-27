import { useCallback, useSyncExternalStore } from "react";
import type { Store } from "./store.ts";

/**
 * Subscribes a component to a slice of a store.
 *
 * `selector` must return either a primitive or a reference that already exists in the
 * state (e.g. `s => s.camp`), never a freshly built object/array, otherwise React would
 * see a new value on every check and re-render forever. Derive collections in the
 * component (with `useMemo` if needed) instead.
 */
export function useStore<S, T>(store: Store<S>, selector: (state: S) => T): T {
  const getSnapshot = useCallback(() => selector(store.get()), [store, selector]);
  return useSyncExternalStore(store.subscribe, getSnapshot, getSnapshot);
}
