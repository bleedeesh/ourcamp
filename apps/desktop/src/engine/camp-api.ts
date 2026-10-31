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
  /** Campfires are voice; chats (ADR-0044) are text and not in the campfire tree. */
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
  /** `open`: an open camp's address, no invite (ADR-0040). */
  readonly kind: "join" | "claim" | "open";
  /**
   * `false` for an old admin key (retired with ADR-0026), or an open camp's address
   * while the camp takes invites only.
   */
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
  /** Chat id → the last message read on this device (ADR-0044). */
  readonly read: Readonly<Record<string, number>>;
}

/** A photo in a message, mirrors `oc_proto::Photo`. */
export interface ChatPhoto {
  readonly hash: string;
  readonly width: number;
  readonly height: number;
}

/** One message of a chat, mirrors `oc_proto::ChatMessage` (ADR-0044). */
export interface ChatMessage {
  readonly channelId: string;
  /** 1, 2, 3… within the chat. */
  readonly seq: number;
  readonly authorId: UserId;
  /** The author's name when they wrote it (they may have left since). */
  readonly authorName: string;
  /** Unix time, milliseconds. */
  readonly sentAt: number;
  /** Empty only with a photo. */
  readonly text: string;
  readonly photo?: ChatPhoto;
  /** Removed (ADR-0048): nothing left to show, the number stays taken. */
  readonly deleted?: boolean;
}

/** A photo to send, encoded by the window (at most 1600 px on its long side). */
export interface OutgoingPhoto {
  readonly bytes: Uint8Array;
  readonly width: number;
  readonly height: number;
}

/** The open camp as the chat window gets it when it opens (ADR-0046). */
export interface ChatContext {
  readonly camp: OpenCamp;
  /** The latest live update about it; `null` until the first one. */
  readonly live: CampLive | null;
}

/** What changed in the open camp's chats, mirrors `camps::ChatUpdate`; every window hears it. */
export type ChatUpdate =
  | { readonly type: "message"; readonly campId: string; readonly message: ChatMessage }
  | { readonly type: "deleted"; readonly campId: string; readonly channelId: string; readonly seq: number }
  | { readonly type: "read"; readonly campId: string; readonly channelId: string; readonly seq: number };

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
  /** Your avatar on this device (ADR-0035), or `null`. */
  profileAvatar(): Promise<Uint8Array | null>;
  /** Sets your avatar (a PNG/WebP from `makeAvatar`) or drops it; the open camp gets it at once. */
  setProfileAvatar(picture: Uint8Array | null): Promise<void>;
  /** A member's avatar by the hash the snapshot names (cached by the app). */
  avatar(id: string, userId: UserId, hash: string): Promise<Uint8Array>;
  /** Kicks someone of a lower rank out of the camp, no ban (ADR-0033). */
  kick(id: string, userId: UserId): Promise<void>;
  /** Forbids someone of a lower rank to speak, or lifts it (ADR-0033). */
  setSilenced(id: string, userId: UserId, silenced: boolean): Promise<void>;
  /** Live state of the camp opened last (who sits where, profiles); returns an unsubscribe. */
  onLive(listener: (live: CampLive) => void): () => void;
  /** What the open camp asks of you alone (ADR-0028); returns an unsubscribe. */
  onDirective(listener: (directive: CampDirective) => void): () => void;
  /** Up to `limit` messages of a chat before `before` (up to the latest when `null`), oldest first. */
  history(id: string, channelId: string, before: number | null, limit: number): Promise<ChatMessage[]>;
  /** Writes in a chat; the message also arrives through `onChat`, like everyone else's. */
  send(id: string, channelId: string, text: string, photo: OutgoingPhoto | null): Promise<ChatMessage>;
  /** Removes a message: your own, or that of someone of a lower rank (ADR-0048). */
  deleteMessage(id: string, channelId: string, seq: number): Promise<void>;
  /** A photo by its hash (kept by the app). */
  photo(id: string, hash: string): Promise<Uint8Array>;
  /** This device has read a chat up to `seq`; never moves back. */
  markRead(id: string, channelId: string, seq: number): Promise<void>;
  /** Messages and read marks of the open camp's chats (ADR-0044); returns an unsubscribe. */
  onChat(listener: (update: ChatUpdate) => void): () => void;
  /** The camp open in the main window, for the chat window (ADR-0046); `null` with none. */
  chatContext(): Promise<ChatContext | null>;
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
    /** Hash of their avatar (ADR-0035); absent without one. */
    readonly avatar?: string;
  }>;
  readonly presence: ReadonlyArray<{
    readonly channelId: string;
    readonly members: ReadonlyArray<{ readonly userId: UserId; readonly micMuted: boolean }>;
  }>;
  /** Chat id → its last message, for chats with any (ADR-0044); absent: none at all. */
  readonly chats?: Readonly<Record<string, number>>;
}

/** A live update about the open camp. */
export interface CampLive {
  readonly campId: string;
  /** The camp's event stream is connected. */
  readonly online: boolean;
  /** Latest snapshot; `null` while offline. */
  readonly state: CampState | null;
}
