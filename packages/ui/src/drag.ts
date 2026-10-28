// Moving people between channels by dragging them (ADR-0029): the pure part.

/** A person being dragged onto a channel. */
export interface Drag {
  readonly userId: string;
  readonly name: string;
  /** The channel they sit in now; dropping them back there does nothing. */
  readonly from: string;
  /** Pointer position, for the ghost under it. */
  readonly x: number;
  readonly y: number;
  /** The channel under the pointer, if any. */
  readonly over: string | null;
}

/** Pointer travel before a press becomes a drag, so a click stays a click. */
export const DRAG_THRESHOLD = 5;

/** Where a drop leads: a channel other than the one they sit in, or nowhere. */
export function dropTarget(from: string, over: string | null): string | null {
  return over !== null && over !== from ? over : null;
}

/** Whether the pointer went far enough from where it was pressed. */
export function pastThreshold(dx: number, dy: number): boolean {
  return Math.hypot(dx, dy) >= DRAG_THRESHOLD;
}

/** Elements a person can be dropped on carry `data-drop-channel="<channel id>"`. */
export const DROP_ATTRIBUTE = "data-drop-channel";
