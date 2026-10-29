import type { ChannelInfo } from "../engine/camp-api.ts";

/** Chat id → a message number (`seq`): the last one there, or the last one read. */
export type SeqByChat = Readonly<Record<string, number>>;

/**
 * Unread messages across a camp's chats (ADR-0044): in each text channel, its last
 * message minus the last one read on this device. Messages are numbered within their
 * chat, so the difference is exact; a chat removed since is not counted.
 */
export function unreadCount(channels: readonly ChannelInfo[], latest: SeqByChat, read: SeqByChat): number {
  let unread = 0;
  for (const channel of channels) {
    if (channel.kind !== "text") continue;
    unread += Math.max(0, (latest[channel.id] ?? 0) - (read[channel.id] ?? 0));
  }
  return unread;
}

/** `seqs` with `chat` at `seq`, unless it is there already or further on. */
export function advance(seqs: SeqByChat, chat: string, seq: number): SeqByChat {
  return (seqs[chat] ?? 0) >= seq ? seqs : { ...seqs, [chat]: seq };
}

/** What the badge says: nothing for none, at most "99+". */
export function badgeText(unread: number): string {
  if (unread <= 0) return "";
  return unread > 99 ? "99+" : String(unread);
}

/** "3 непрочитанных", "1 непрочитанное", for screen readers. */
export function unreadLabel(n: number): string {
  const mod10 = n % 10;
  const mod100 = n % 100;
  if (mod10 === 1 && mod100 !== 11) return `${n} непрочитанное`;
  return `${n} непрочитанных`;
}
