import type { Role, UserId } from "../domain/types.ts";

/** A joined camp, as listed in the switcher. */
export interface CampSummary {
  /** Stable id (the camp's certificate fingerprint). */
  readonly id: string;
  readonly name: string;
  readonly role: Role;
}

/** A channel as the camp describes it, mirrors `oc_proto::Channel` (ADR-0026). */
export interface ChannelInfo {
  readonly id: string;
  readonly name: string;
  /** Campfires are voice; chats (M5) are text and not in the campfire tree. */
  readonly kind: "voice" | "text";
  readonly afk: boolean;
}

export interface CampList {
  /** Name the user goes by; prefills the join form. */
  readonly displayName: string;
  readonly camps: readonly CampSummary[];
  /** Camp to open on start. */
  readonly current: string | null;
}

/** What an invite link leads to, shown before joining. */
export interface CampPreview {
  readonly name: string;
  /** Number of campfires. */
  readonly channels: number;
  readonly kind: "join" | "claim";
  /** `false` for an old admin key: retired with ADR-0026, it lets nobody in. */
  readonly usable: boolean;
}

/** An opened camp. */
export interface OpenCamp {
  readonly id: string;
  readonly name: string;
  readonly channels: readonly ChannelInfo[];
  readonly self: { readonly userId: UserId; readonly name: string; readonly role: Role };
  /** The camp server was unreachable; name and channels are the last known ones. */
  readonly offline: boolean;
}

/**
 * Camps this device belongs to (ADR-0020). In the desktop app this is Rust
 * (`src-tauri/src/camps.rs`): it holds the device key and talks to camps over pinned
 * TLS. Every method rejects with a sentence ready to show the user.
 */
export interface CampApi {
  list(): Promise<CampList>;
  preview(link: string): Promise<CampPreview>;
  join(link: string, name: string): Promise<OpenCamp>;
  open(id: string): Promise<OpenCamp>;
  /**
   * Leaves a camp: the camp forgets you, then this device forgets the camp. `false` when
   * the camp was unreachable — it is gone from the list here, but there you still count.
   */
  leave(id: string): Promise<boolean>;
  /** The camp's friends link, to copy. */
  invite(id: string): Promise<string>;
  /** Moves someone sitting in voice to another campfire; needs the rights (ADR-0029). */
  moveMember(id: string, userId: UserId, channelId: string): Promise<void>;
  /** Kicks someone of a lower rank out of the camp, no ban (ADR-0033). */
  kick(id: string, userId: UserId): Promise<void>;
  /** Forbids someone of a lower rank to speak, or lifts it (ADR-0033). */
  setSilenced(id: string, userId: UserId, silenced: boolean): Promise<void>;
  /** Live state of the camp opened last (who sits where, profiles); returns an unsubscribe. */
  onLive(listener: (live: CampLive) => void): () => void;
  /** What the open camp asks of you alone (ADR-0028); returns an unsubscribe. */
  onDirective(listener: (directive: CampDirective) => void): () => void;
}

/**
 * A moderator's word to you alone (ADR-0028), mirrors `camps::CampDirective`. On
 * `kicked` this device has already forgotten the camp.
 */
export type CampDirective =
  | { readonly type: "moved"; readonly campId: string; readonly channelId: string }
  | { readonly type: "kicked"; readonly campId: string; readonly campName: string };

/** The camp's live state (ADR-0021, 0026), mirrors `oc_proto::CampState`. */
export interface CampState {
  /** Renames and channel changes arrive here too. */
  readonly name: string;
  readonly channels: readonly ChannelInfo[];
  readonly members: ReadonlyArray<{
    readonly userId: UserId;
    readonly name: string;
    readonly role: Role;
    /** Forbidden to speak until lifted (ADR-0028). */
    readonly silenced: boolean;
  }>;
  readonly presence: ReadonlyArray<{
    readonly channelId: string;
    readonly members: ReadonlyArray<{ readonly userId: UserId; readonly micMuted: boolean }>;
  }>;
}

/** A live update about the open camp. */
export interface CampLive {
  readonly campId: string;
  /** The camp's event stream is connected. */
  readonly online: boolean;
  /** Latest snapshot; `null` while offline. */
  readonly state: CampState | null;
}
