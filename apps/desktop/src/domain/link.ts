import type { LinkStatus } from "../engine/engine.ts";

/** The connection line of the bottom panel (ADR-0030). */
export interface LinkView {
  /** "Отличная связь", "Подключение…". */
  readonly label: string;
  /** "24 мс", or null before the first measurement. */
  readonly ping: string | null;
  /** Lit bars of three. */
  readonly bars: 0 | 1 | 2 | 3;
  readonly tone: "good" | "weak" | "bad" | "pending";
}

/** What the panel shows for the engine's report; `null` — just joined, nothing measured. */
export function linkView(link: LinkStatus | null): LinkView {
  const rtt = link?.rttMs ?? null;
  // A server on the same machine or LAN answers in under a millisecond.
  const ping = rtt === null ? null : rtt < 1 ? "<1 мс" : `${rtt} мс`;
  switch (link?.quality) {
    case "excellent":
      return { label: "Отличная связь", ping, bars: 3, tone: "good" };
    case "good":
      return { label: "Хорошая связь", ping, bars: 2, tone: "good" };
    case "poor":
      return { label: "Слабая связь", ping, bars: 1, tone: "weak" };
    case "lost":
      return { label: "Связь потеряна", ping: null, bars: 0, tone: "bad" };
    case undefined:
      return { label: "Подключение…", ping: null, bars: 0, tone: "pending" };
  }
}
