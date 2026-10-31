/** A member's role in a camp (ADR-0026); the owner runs the server and is no member. */
export type Role = "member" | "moderator" | "admin";

/** Stable account id issued by the accounts service. */
export type UserId = string;

export interface Member {
  readonly id: UserId;
  readonly name: string;
  /** Avatar background (until real avatars exist). */
  readonly color: string;
  /** The member muted their own microphone. */
  readonly micMuted: boolean;
  /** The member turned their sound off. */
  readonly deafened: boolean;
  /** A moderator forbade them to speak (ADR-0028); the camp's LiveKit enforces it. */
  readonly silenced: boolean;
  /** Role in the camp: a crown for admins, a shield for moderators. */
  readonly role: Role;
  /** The member is streaming their screen. */
  readonly streaming: boolean;
  /** Hash of their avatar on the camp (ADR-0035); `null`: the initial instead. */
  readonly avatar: string | null;
}

/** A voice channel ("костёр"). */
export interface Channel {
  readonly id: string;
  readonly name: string;
  /** The system AFK campfire: mic and sound go off inside (ADR-0025). */
  readonly afk: boolean;
  /** Members currently connected, excluding the local user (added by the UI). */
  readonly memberIds: readonly UserId[];
}

/** A camp (self-hosted server). */
export interface Camp {
  readonly id: string;
  readonly name: string;
  /** Two-letter badge shown when the camp has no icon. */
  readonly short: string;
  readonly color: string;
  readonly channels: readonly Channel[];
  readonly members: Readonly<Record<UserId, Member>>;
}

/** Who an avatar shows: a camp member in the tree, an author in the chat, a streamer on the panel. */
export interface AvatarPerson {
  readonly id: string;
  readonly name: string;
  readonly color: string;
  /** Avatar hash (ADR-0035); `null`: the initial on their colour. */
  readonly avatar: string | null;
}
