import { listen } from "@tauri-apps/api/event";

/** Subscribes to a Tauri event; returns an unsubscribe that also works before `listen` resolves. */
export function subscribe<T>(event: string, listener: (payload: T) => void): () => void {
  let disposed = false;
  let unlisten: (() => void) | null = null;
  void listen<T>(event, (e) => listener(e.payload)).then((fn) => {
    // The subscription resolves asynchronously; honour an unsubscribe that came first.
    if (disposed) fn();
    else unlisten = fn;
  });
  return () => {
    disposed = true;
    unlisten?.();
  };
}
