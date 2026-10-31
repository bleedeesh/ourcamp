/** The numbers of the viewer's panel, as Rust reports them (ADR-0053). */
export interface ViewStatus {
  /** Pictures are arriving. */
  readonly live: boolean;
  /** Height of the picture. */
  readonly height: number;
  /** Pictures in the last second. */
  readonly fps: number;
  /** How many are watching, you included. */
  readonly viewers: number;
}

/** Who the window shows, and how it is going. */
export interface StreamView {
  readonly userId: string;
  readonly name: string;
  readonly status: ViewStatus;
}

/** "1080p · 60 fps"; empty until there is a picture to speak of. */
export function qualityText(status: ViewStatus): string {
  if (!status.live || status.height === 0) return "";
  return `${status.height}p · ${status.fps} fps`;
}

/** "1 смотрит", "2 смотрят", "21 смотрит": the verb agrees with the number. */
export function viewersText(viewers: number): string {
  const one = viewers % 10 === 1 && viewers % 100 !== 11;
  return `${viewers} ${one ? "смотрит" : "смотрят"}`;
}

/** What the page shows in the picture's place while there is no picture. */
export function waitingText(status: ViewStatus, seen: boolean): string {
  if (status.live) return "";
  return seen ? "Стрим закончился" : "Подключаемся к стриму…";
}

/** The picture's rectangle in the window's pixels, from the page's CSS pixels. */
export function physicalRect(
  rect: { readonly left: number; readonly top: number; readonly width: number; readonly height: number },
  scale: number,
): { x: number; y: number; width: number; height: number } {
  const x = Math.round(rect.left * scale);
  const y = Math.round(rect.top * scale);
  // From the rounded edges: two neighbours never leave a gap between them.
  return {
    x,
    y,
    width: Math.round((rect.left + rect.width) * scale) - x,
    height: Math.round((rect.top + rect.height) * scale) - y,
  };
}
