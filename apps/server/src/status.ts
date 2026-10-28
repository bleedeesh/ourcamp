// The window as data: mirrors `ServerStatus` of src-tauri/src/status.rs (ADR-0023).

export type Phase = { kind: "starting" } | { kind: "running" } | { kind: "failed"; message: string };

export interface Member {
  userId: string;
  name: string;
  isAdmin: boolean;
  /** Campfire they sit at; null when not in voice. */
  campfire: string | null;
  micMuted: boolean;
}

export interface ServerStatus {
  phase: Phase;
  campName: string;
  address: string | null;
  fingerprint: string | null;
  joinLink: string | null;
  /** Only while nobody is admin yet. */
  claimLink: string | null;
  /** Admins first, then by name. */
  members: Member[];
  livekitPid: number | null;
}

export const STARTING: ServerStatus = {
  phase: { kind: "starting" },
  campName: "",
  address: null,
  fingerprint: null,
  joinLink: null,
  claimLink: null,
  members: [],
  livekitPid: null,
};

/** The line under the logo. */
export function headline(status: ServerStatus): string {
  switch (status.phase.kind) {
    case "starting":
      return "Лагерь запускается…";
    case "failed":
      return "Лагерь не запустился";
    case "running":
      return "Лагерь работает";
  }
}

/** "3 в лагере · 1 у костра" — or a hint while nobody came yet. */
export function crowd(members: readonly Member[]): string {
  if (members.length === 0) return "Пока никого — отправьте друзьям ссылку";
  const seated = members.filter((m) => m.campfire !== null).length;
  const people = `${members.length} ${plural(members.length, "человек", "человека", "человек")} в лагере`;
  return seated === 0 ? people : `${people} · ${seated} у костра`;
}

/** Voice status for the footer. */
export function voice(status: ServerStatus): string {
  if (status.phase.kind !== "running") return "Голос: —";
  return status.livekitPid === null ? "Голос: перезапускается…" : `Голос: работает (LiveKit, pid ${status.livekitPid})`;
}

/** Russian plural: 1 человек, 2 человека, 5 человек. */
export function plural(n: number, one: string, few: string, many: string): string {
  const tens = n % 100;
  const units = n % 10;
  if (tens >= 11 && tens <= 14) return many;
  if (units === 1) return one;
  if (units >= 2 && units <= 4) return few;
  return many;
}
