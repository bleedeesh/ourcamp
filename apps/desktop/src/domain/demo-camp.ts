import type { Camp, Member } from "./types.ts";

/**
 * Sample camp used until the camp server and accounts service exist (roadmap M3/M4).
 * Same people and channels as the design prototype.
 */
const m = (id: string, name: string, color: string, extra: Partial<Member> = {}): Member => ({
  id,
  name,
  color,
  micMuted: false,
  deafened: false,
  isAdmin: false,
  streaming: false,
  ...extra,
});

export const DEMO_SELF_ID = "dima";

export const DEMO_CAMP: Camp = {
  id: "vk",
  name: "Вечерний кооп",
  short: "ВК",
  color: "#8E44AD",
  members: {
    dima: m("dima", "Дима", "#4FC3F7"),
    artem: m("artem", "Артём", "#6C8CFF", { isAdmin: true }),
    lena: m("lena", "Лена", "#F2A65A", { streaming: true }),
    max: m("max", "Макс", "#C792EA", { micMuted: true }),
    olya: m("olya", "Оля", "#8FD694"),
    kirill: m("kirill", "Кирилл", "#7FD1C7", { deafened: true }),
  },
  channels: [
    { id: "lobby", name: "Лобби", memberIds: ["artem", "lena", "max"] },
    { id: "katka", name: "Катка", memberIds: ["olya", "kirill"] },
    { id: "afk", name: "Отошёл (AFK)", memberIds: [] },
    { id: "quiet", name: "Тихий час", memberIds: [] },
  ],
};
