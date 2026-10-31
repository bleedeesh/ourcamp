import type { ChatMessage } from "../engine/camp-api.ts";

/**
 * Messages of one chat as the window lays them out (ADR-0046): oldest first, a line for
 * each day, the author's name only where they start talking, and a "new" line before
 * the first message that was unread when the chat opened. Removed messages (ADR-0048)
 * do not show: nothing is left of them but their number.
 */
export type FeedItem =
  | { readonly kind: "day"; readonly key: string; readonly label: string }
  | { readonly kind: "unread"; readonly key: string }
  | {
      readonly kind: "message";
      readonly key: string;
      readonly message: ChatMessage;
      /** Shows the avatar, the name and the time: a new speaker, or a pause. */
      readonly head: boolean;
    };

/** Messages of one author closer than this read as one turn. */
const SAME_TURN_MS = 5 * 60 * 1000;

/** `existing` and `incoming` together, by `seq`, each once (the newer copy wins). */
export function mergeMessages(existing: readonly ChatMessage[], incoming: readonly ChatMessage[]): ChatMessage[] {
  if (incoming.length === 0) return [...existing];
  const bySeq = new Map<number, ChatMessage>();
  for (const message of existing) bySeq.set(message.seq, message);
  for (const message of incoming) bySeq.set(message.seq, message);
  return [...bySeq.values()].sort((a, b) => a.seq - b.seq);
}

/** The numbers missing between the first message held and `upTo`: what to fetch. */
export function gapBefore(messages: readonly ChatMessage[], upTo: number): number {
  const last = messages.at(-1)?.seq ?? 0;
  return Math.max(0, upTo - last - 1);
}

function dayKey(time: number): string {
  const d = new Date(time);
  return `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`;
}

const MONTHS = [
  "января",
  "февраля",
  "марта",
  "апреля",
  "мая",
  "июня",
  "июля",
  "августа",
  "сентября",
  "октября",
  "ноября",
  "декабря",
];

/** "Сегодня", "Вчера", "12 октября", "3 марта 2025" (another year). */
export function dayLabel(time: number, now: number): string {
  const day = new Date(time);
  const today = new Date(now);
  if (dayKey(time) === dayKey(now)) return "Сегодня";
  const yesterday = new Date(today.getFullYear(), today.getMonth(), today.getDate() - 1);
  if (dayKey(time) === dayKey(yesterday.getTime())) return "Вчера";
  const date = `${day.getDate()} ${MONTHS[day.getMonth()]}`;
  return day.getFullYear() === today.getFullYear() ? date : `${date} ${day.getFullYear()}`;
}

/** "21:07". */
export function timeLabel(time: number): string {
  const d = new Date(time);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

/**
 * The feed of `messages` (oldest first). `unreadAfter`: the last message read when the
 * chat was opened — the "new" line goes before the first later one by someone else;
 * `null` for none.
 */
export function layoutFeed(
  messages: readonly ChatMessage[],
  now: number,
  selfId: string,
  unreadAfter: number | null,
): FeedItem[] {
  const items: FeedItem[] = [];
  let previous: ChatMessage | null = null;
  let unreadShown = false;
  for (const message of messages) {
    if (message.deleted) continue;
    const newDay = previous === null || dayKey(previous.sentAt) !== dayKey(message.sentAt);
    if (newDay) items.push({ kind: "day", key: `day-${dayKey(message.sentAt)}`, label: dayLabel(message.sentAt, now) });
    const unread =
      !unreadShown && unreadAfter !== null && message.seq > unreadAfter && message.authorId !== selfId;
    if (unread) {
      unreadShown = true;
      items.push({ kind: "unread", key: "unread" });
    }
    const sameTurn =
      previous !== null &&
      !newDay &&
      !unread &&
      previous.authorId === message.authorId &&
      message.sentAt - previous.sentAt < SAME_TURN_MS;
    items.push({ kind: "message", key: `m-${message.seq}`, message, head: !sameTurn });
    previous = message;
  }
  return items;
}
