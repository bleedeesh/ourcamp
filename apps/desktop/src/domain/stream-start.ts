import type { CampState } from "../engine/camp-api.ts";
import type { UserId } from "./types.ts";

/** A screen or window to offer, mirrors `stream::StreamSource`. */
export interface StreamSource {
  readonly kind: "screen" | "window";
  readonly id: number;
  readonly title: string;
}

/** A quality to offer, mirrors `stream::QualityOption`. Rust holds the table. */
export interface QualityOption {
  readonly height: number;
  readonly fps: number;
  /** Bits per second. */
  readonly bitrate: number;
}

/** A tile of "Что показать". */
export interface SourceTile {
  /** Stable while the source lasts, unlike its place in the list. */
  readonly key: string;
  readonly source: StreamSource;
  readonly label: string;
}

/** Screens by number, then windows by title. A window without a title is not one to show. */
export function sourceTiles(sources: readonly StreamSource[]): SourceTile[] {
  const screens = sources.filter((s) => s.kind === "screen");
  const windows = sources.filter((s) => s.kind === "window" && s.title.trim() !== "");
  return [
    ...screens.map((source, i) => ({ key: `screen:${source.id}`, source, label: `Весь экран ${i + 1}` })),
    ...windows.map((source) => ({ key: `window:${source.id}`, source, label: `Окно: ${source.title.trim()}` })),
  ];
}

/** The choice as the list changes: what was chosen while it lasts, else the first. */
export function keepChoice(tiles: readonly SourceTile[], chosen: string | null): string | null {
  if (tiles.some((t) => t.key === chosen)) return chosen;
  return tiles[0]?.key ?? null;
}

/** The distinct values of one column of the table, in its order. */
export function columnOf(options: readonly QualityOption[], column: "height" | "fps"): number[] {
  return [...new Set(options.map((o) => o[column]))];
}

/** What a quality costs per viewer, bits per second; 0 when the table does not have it. */
export function bitrateOf(options: readonly QualityOption[], height: number, fps: number): number {
  return options.find((o) => o.height === height && o.fps === fps)?.bitrate ?? 0;
}

/** Your channel as the camp's snapshot tells it. */
export interface ChannelNow {
  /** Everyone else there: all of them may watch. */
  readonly others: number;
  /** Those of them on air. */
  readonly onAir: ReadonlyArray<{ readonly userId: UserId; readonly name: string }>;
}

/** Nobody around, as far as is known: the snapshot may be a moment behind the room. */
export const EMPTY_CHANNEL: ChannelNow = { others: 0, onAir: [] };

export function channelNow(state: CampState | null, selfId: UserId): ChannelNow {
  const mine = state?.presence.find((c) => c.members.some((m) => m.userId === selfId));
  if (!state || !mine) return EMPTY_CHANNEL;
  const others = mine.members.filter((m) => m.userId !== selfId);
  return {
    others: others.length,
    onAir: others
      .filter((m) => m.streaming === true)
      .map((m) => ({
        userId: m.userId,
        name: state.members.find((info) => info.userId === m.userId)?.name ?? m.userId,
      })),
  };
}

/** "2,5" and "10": Мбит/с the Russian way. */
export function mbitText(bitsPerSecond: number): string {
  return String(bitsPerSecond / 1_000_000).replace(".", ",");
}

/**
 * The warning of ADR-0049: a home server sends the stream to each viewer over its host's
 * uplink, and there is no simulcast to spare it. `others` is everyone who may watch.
 */
export function uploadText(bitsPerSecond: number, others: number): string {
  const each = `Сервер отправляет стрим каждому зрителю отдельно, до ${mbitText(bitsPerSecond)} Мбит/с.`;
  if (others === 0) return `${each} На домашнем сервере это исходящий канал хозяина.`;
  const one = others % 10 === 1 && others % 100 !== 11;
  const total = mbitText(bitsPerSecond * others);
  return `${each} При ${others} ${one ? "зрителе" : "зрителях"} это до ${total} Мбит/с исходящего канала; на домашнем сервере — канала хозяина.`;
}

/** 1440p is measured on a fast video card only (ADR-0054): the streamer should know. */
export function heavyText(height: number): string | null {
  return height >= 1440 ? "1440p нагружает компьютер сильнее: на слабом частота кадров у зрителей упадёт." : null;
}
