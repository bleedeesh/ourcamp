import type { UserId } from "../domain/types.ts";

/** Per-member live audio info pushed by the engine ~20 times per second. */
export interface MemberLevel {
  readonly userId: UserId;
  /** Pre-volume level in dBFS (how loud the person is, regardless of our slider). */
  readonly levelDb: number;
  readonly speaking: boolean;
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
}
