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
  /** The stream comes with sound. */
  readonly sound: boolean;
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

/** What the volume of streams starts at, as in the mockup. */
export const DEFAULT_STREAM_VOLUME = 70;

const STREAM_VOLUME_KEY = "ourcamp.streamVolume";

/** The volume of streams as last set on this device; browser storage may fail, or lie. */
export function readStreamVolume(storage: Pick<Storage, "getItem">): number {
  try {
    const stored = storage.getItem(STREAM_VOLUME_KEY);
    const volume = stored === null ? Number.NaN : Number(stored);
    return Number.isFinite(volume) && volume >= 0 && volume <= 100 ? volume : DEFAULT_STREAM_VOLUME;
  } catch {
    return DEFAULT_STREAM_VOLUME;
  }
}

/** A failed write costs only the memory of this value. */
export function saveStreamVolume(storage: Pick<Storage, "setItem">, volume: number): void {
  try {
    storage.setItem(STREAM_VOLUME_KEY, String(volume));
  } catch {
    // Nothing to do: the sound itself is already at this volume.
  }
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
