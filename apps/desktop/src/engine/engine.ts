import type { UserId } from "../domain/types.ts";

/** Per-member live audio info pushed by the engine ~20 times per second. */
export interface MemberLevel {
  readonly userId: UserId;
  /** Pre-volume level in dBFS (how loud the person is, regardless of our slider). */
  readonly levelDb: number;
  readonly speaking: boolean;
}

/** A remote member of the joined channel, as the voice room reports them. */
export interface RemoteParticipant {
  readonly userId: UserId;
  /** Display name carried by the room (used for people the camp does not know yet). */
  readonly name: string;
  readonly micMuted: boolean;
}

/**
 * Who is in the joined channel (ADR-0011). Always the complete list, so a missed
 * update never leaves the UI out of sync.
 */
export interface ParticipantsUpdate {
  /** The local user's id as the room knows it. */
  readonly selfId: UserId;
  readonly members: readonly RemoteParticipant[];
}

/**
 * Our connection to the voice server while in a channel (ADR-0030): LiveKit's quality
 * rating and the round trip of the media path.
 */
export interface LinkStatus {
  readonly quality: "excellent" | "good" | "poor" | "lost";
  /** Ping in ms; null until ICE has measured it. */
  readonly rttMs: number | null;
}

export interface JoinRequest {
  readonly campId: string;
  readonly channelId: string;
  /**
   * Remote members already in the channel, with this listener's saved volumes.
   * `micMuted` is the member's own mute state as reported by the camp server.
   */
  readonly members: ReadonlyArray<{ userId: UserId; volume: number; muted: boolean; micMuted: boolean }>;
}

/**
 * Boundary between the React UI and the native media engine.
 *
 * In the desktop app this is implemented by Rust (`src-tauri/src/commands.rs`): audio
 * never passes through the webview. The demo implementation lets the UI run in a
 * plain browser for layout work.
 */
export interface VoiceEngine {
  joinChannel(request: JoinRequest): Promise<void>;
  leaveChannel(): Promise<void>;
  /** Local listening volume of one member, 0–200 %. */
  setVolume(userId: UserId, percent: number): Promise<void>;
  /** Silence one member for this listener only. */
  setLocalMute(userId: UserId, muted: boolean): Promise<void>;
  setMicMuted(muted: boolean): Promise<void>;
  setDeafened(deafened: boolean): Promise<void>;
  /** Subscribes to level updates; returns an unsubscribe function. */
  onLevels(listener: (levels: readonly MemberLevel[]) => void): () => void;
  /** Subscribes to channel membership updates; returns an unsubscribe function. */
  onParticipants(listener: (update: ParticipantsUpdate) => void): () => void;
  /**
   * The server ended the voice session (same person joined elsewhere, campfire closed,
   * connection lost). `reason` is a sentence for the user. Returns an unsubscribe function.
   */
  onDisconnected(listener: (reason: string) => void): () => void;
  /** Connection quality and ping, on change, while in a channel. Returns an unsubscribe function. */
  onLink(listener: (link: LinkStatus) => void): () => void;
}
