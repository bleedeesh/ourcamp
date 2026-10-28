import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";

import { DROP_ATTRIBUTE, dropTarget, pastThreshold, type Drag } from "./drag.ts";

export type { Drag } from "./drag.ts";

/** Who is picked up: the person and the channel they sit in. */
export interface Draggable {
  readonly userId: string;
  readonly name: string;
  readonly from: string;
}

function channelAt(x: number, y: number): string | null {
  const target = document.elementFromPoint(x, y)?.closest(`[${DROP_ATTRIBUTE}]`);
  return target?.getAttribute(DROP_ATTRIBUTE) ?? null;
}

/** The click a browser sends after a drag ends over the pressed element is no click. */
function swallowNextClick(): void {
  const swallow = (e: MouseEvent) => {
    e.stopPropagation();
    e.preventDefault();
  };
  window.addEventListener("click", swallow, { capture: true, once: true });
  setTimeout(() => window.removeEventListener("click", swallow, { capture: true }), 0);
}

/**
 * Drag a person onto a channel to move them there (ADR-0029). Pointer events rather
 * than HTML drag-and-drop: the same in every webview (Tauri's file drop takes HTML5 drops
 * on Windows) and a ghost of our own. Drop targets are elements with
 * `data-drop-channel`; Escape cancels.
 *
 * `grab(who)` goes on the person's `onPointerDown`; `drag` is what to draw meanwhile.
 */
export function useDragMove(onDrop: (userId: string, channelId: string) => void): {
  drag: Drag | null;
  grab: (who: Draggable) => (e: ReactPointerEvent<HTMLElement>) => void;
} {
  const [drag, setDrag] = useState<Drag | null>(null);
  const onDropRef = useRef(onDrop);
  const stopRef = useRef<(() => void) | null>(null);
  useEffect(() => {
    onDropRef.current = onDrop;
  }, [onDrop]);
  // A drag in flight when the list unmounts must not keep window listeners.
  useEffect(() => () => stopRef.current?.(), []);

  const grab = (who: Draggable) => (e: ReactPointerEvent<HTMLElement>) => {
    if (e.button !== 0 || !e.isPrimary) return;
    stopRef.current?.();
    const startX = e.clientX;
    const startY = e.clientY;
    let current: Drag | null = null;

    const move = (ev: PointerEvent) => {
      if (current === null && !pastThreshold(ev.clientX - startX, ev.clientY - startY)) return;
      current = { ...who, x: ev.clientX, y: ev.clientY, over: channelAt(ev.clientX, ev.clientY) };
      setDrag(current);
    };
    const finish = (drop: boolean) => {
      stop();
      if (current === null) return;
      const to = drop ? dropTarget(current.from, current.over) : null;
      setDrag(null);
      swallowNextClick();
      if (to !== null) onDropRef.current(current.userId, to);
    };
    const up = () => finish(true);
    const cancel = () => finish(false);
    const key = (ev: KeyboardEvent) => {
      if (ev.key === "Escape") finish(false);
    };
    const stop = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", cancel);
      window.removeEventListener("keydown", key);
      stopRef.current = null;
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", cancel);
    window.addEventListener("keydown", key);
    stopRef.current = stop;
  };

  return { drag, grab };
}

/** The person under the pointer while dragged. */
export function DragGhost({ drag }: { drag: Drag | null }) {
  if (drag === null) return null;
  return (
    <div
      className="drag-ghost"
      data-ok={dropTarget(drag.from, drag.over) !== null || undefined}
      style={{ transform: `translate(${drag.x + 12}px, ${drag.y + 8}px)` }}
      aria-hidden="true"
    >
      {drag.name}
    </div>
  );
}
