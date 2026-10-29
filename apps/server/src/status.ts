// The window as data: mirrors `ServerStatus` of src-tauri/src/status.rs (ADR-0023, 0026).

/** A member's role (ADR-0026); the owner — whoever runs this window — is no member. */
export type Role = "member" | "moderator" | "admin";

/** A channel of the camp: a campfire or a chat. */
export interface Channel {
  id: string;
  name: string;
  kind: "voice" | "text";
  afk: boolean;
}

export type Phase = { kind: "starting" } | { kind: "running" } | { kind: "failed"; message: string };

export interface Member {
  userId: string;
  name: string;
  role: Role;
  /** Campfire they sit at; null when not in voice. */
  campfire: string | null;
  /** Its id, to move them elsewhere (ADR-0028). */
  campfireId: string | null;
  micMuted: boolean;
  /** Forbidden to speak until lifted (ADR-0028). */
  silenced: boolean;
  /** Hash of their avatar (ADR-0035); null without one. */
  avatar: string | null;
}

export interface ServerStatus {
  phase: Phase;
  campName: string;
  address: string | null;
  fingerprint: string | null;
  joinLink: string | null;
  /** Campfires and chats, in display order. */
  channels: Channel[];
  /** Admins first, then moderators, then by name. */
  members: Member[];
  livekitPid: number | null;
}

export const STARTING: ServerStatus = {
  phase: { kind: "starting" },
  campName: "",
  address: null,
  fingerprint: null,
  joinLink: null,
  channels: [],
  members: [],
  livekitPid: null,
};

/** The line under the logo. */
export function headline(status: ServerStatus): string {
  switch (status.phase.kind) {
    case "starting":
      return "Сервер запускается…";
    case "failed":
      return "Сервер не запустился";
    case "running":
      return "Сервер работает";
  }
}

/** "3 в лагере · 1 у костра" — or a hint while nobody came yet. */
export function crowd(members: readonly Member[]): string {
  if (members.length === 0) return "Пока никого — отправьте друзьям ссылку";
  const seated = members.filter((m) => m.campfire !== null).length;
  const people = `${members.length} ${plural(members.length, "человек", "человека", "человек")} на сервере`;
  return seated === 0 ? people : `${people} · ${seated} в голосе`;
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

/** Same limits as `oc_server::MAX_CAMP_NAME` / `MAX_CHANNEL_NAME`. */
export const MAX_CAMP_NAME = 48;
export const MAX_CHANNEL_NAME = 32;

/**
 * What is wrong with a name before it goes to the camp (which checks the same), or
 * null when it is fine. Counted in characters after trimming, as the server does.
 */
export function nameProblem(name: string, max: number): string | null {
  const length = [...name.trim()].length;
  if (length === 0) return "Введите название";
  if (length > max) return `Не длиннее ${max} символов`;
  return null;
}

/** Roles as the window names them. */
export const ROLE_LABEL: Record<Role, string> = {
  admin: "Админ",
  moderator: "Модератор",
  member: "Участник",
};
