import type { ParticipantsUpdate } from "../engine/engine.ts";
import type { Camp, Member, UserId } from "./types.ts";

/** Avatar backgrounds for people the camp has no profile for (until accounts, M4). */
const AVATAR_COLORS = ["#6C8CFF", "#F2A65A", "#C792EA", "#8FD694", "#7FD1C7", "#4FC3F7", "#F28FAD"];

/** Same colour for the same person every time (FNV-1a of the id). */
export function avatarColor(userId: UserId): string {
  let hash = 0x811c9dc5;
  for (let i = 0; i < userId.length; i++) {
    hash = Math.imul(hash ^ userId.charCodeAt(i), 0x01000193) >>> 0;
  }
  return AVATAR_COLORS[hash % AVATAR_COLORS.length] ?? "#888888";
}

function newcomer(id: UserId, name: string): Member {
  return {
    id,
    name: name.trim() === "" ? id : name,
    color: avatarColor(id),
    micMuted: false,
    deafened: false,
    isAdmin: false,
    streaming: false,
  };
}

/**
 * Applies the voice room's view of `channelId` to the camp (ADR-0011): the channel
 * holds exactly the room's members, each person sits in one channel only, unknown
 * people get a profile from the room, and mic state follows the room.
 */
export function applyRoster(camp: Camp, channelId: string, update: ParticipantsUpdate): Camp {
  const members: Record<UserId, Member> = { ...camp.members };
  const self = members[update.selfId];
  if (!self) members[update.selfId] = newcomer(update.selfId, update.selfId);

  for (const p of update.members) {
    const known = members[p.userId];
    members[p.userId] = known ? { ...known, micMuted: p.micMuted } : { ...newcomer(p.userId, p.name), micMuted: p.micMuted };
  }

  const here = new Set<UserId>([update.selfId, ...update.members.map((m) => m.userId)]);
  const channels = camp.channels.map((c) =>
    c.id === channelId
      ? { ...c, memberIds: update.members.map((m) => m.userId) }
      : { ...c, memberIds: c.memberIds.filter((id) => !here.has(id)) },
  );
  return { ...camp, members, channels };
}
