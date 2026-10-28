import type { LinkStatus } from "../engine/engine.ts";

/** The connection line of the bottom panel (ADR-0030): the ping, bars for the quality. */
export interface LinkView {
  /** "10 ms"; "Подключение…" before the first measurement, "Нет связи" when lost. */
  readonly text: string;
  /** The quality in words, for the tooltip and screen readers. */
  readonly quality: string;
  /** Lit bars of three. */
  readonly bars: 0 | 1 | 2 | 3;
  readonly tone: "good" | "weak" | "bad" | "pending";
}

const QUALITY = {
  excellent: { quality: "Отличная связь", bars: 3, tone: "good" },
  good: { quality: "Хорошая связь", bars: 2, tone: "good" },
  poor: { quality: "Слабая связь", bars: 1, tone: "weak" },
  lost: { quality: "Связь потеряна", bars: 0, tone: "bad" },
} as const;

/** What the panel shows for the engine's report; `null` — just joined, nothing measured. */
export function linkView(link: LinkStatus | null): LinkView {
  if (link === null) return { text: "Подключение…", quality: "Связь проверяется", bars: 0, tone: "pending" };
  const { quality, bars, tone } = QUALITY[link.quality];
  if (link.quality === "lost") return { text: "Нет связи", quality, bars, tone };
  // A server on the same machine or LAN answers in under a millisecond.
  const text = link.rttMs === null ? "Подключение…" : link.rttMs < 1 ? "<1 ms" : `${link.rttMs} ms`;
  return { text, quality, bars, tone };
}
