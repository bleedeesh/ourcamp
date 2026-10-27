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
  /** Camp admin (holder of the redeemed claim key). */
  readonly isAdmin: boolean;
  /** The member is streaming their screen. */
  readonly streaming: boolean;
}

/** A voice channel ("костёр"). */
export interface Channel {
  readonly id: string;
  readonly name: string;
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
