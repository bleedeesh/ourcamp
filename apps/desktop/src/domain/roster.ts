import type { CampState } from "../engine/camp-api.ts";
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

/**
 * Applies the camp server's snapshot (ADR-0021): profiles (names, crowns) for everyone,
 * and who sits at every campfire — except the one you are in, whose room reports it
 * sooner and more precisely (`applyRoster`). You are never listed by presence: the tree
 * adds you to your own campfire itself.
 */
export function applyCampState(camp: Camp, state: CampState, currentChannelId: string | null, selfId: UserId): Camp {
  const members: Record<UserId, Member> = { ...camp.members };
  for (const info of state.members) {
    const known = members[info.userId] ?? newcomer(info.userId, info.name);
    members[info.userId] = { ...known, name: info.name, isAdmin: info.isAdmin };
  }
  const presence = new Map(state.presence.map((p) => [p.channelId, p.members]));
  const roomMembers = new Set(camp.channels.find((c) => c.id === currentChannelId)?.memberIds ?? []);
  const channels = camp.channels.map((c) => {
    if (c.id === currentChannelId) return c;
    const here = (presence.get(c.id) ?? []).filter((p) => p.userId !== selfId && !roomMembers.has(p.userId));
    for (const p of here) {
      const m = members[p.userId] ?? newcomer(p.userId, p.userId);
      members[p.userId] = { ...m, micMuted: p.micMuted };
    }
    return { ...c, memberIds: here.map((p) => p.userId) };
  });
  return { ...camp, members, channels };
}
