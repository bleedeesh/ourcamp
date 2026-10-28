import { invoke } from "@tauri-apps/api/core";
import type { UserId } from "../domain/types.ts";
import type { JoinRequest, MemberLevel, ParticipantsUpdate, VoiceEngine } from "./engine.ts";
import { subscribe } from "./tauri-events.ts";

/** Event emitted by `src-tauri/src/levels.rs`. */
const LEVELS_EVENT = "voice://levels";
/** Event emitted by `src-tauri/src/participants.rs`. */
const PARTICIPANTS_EVENT = "voice://participants";
/** Event emitted by `src-tauri/src/participants.rs` when the server ends a session. */
const DISCONNECTED_EVENT = "voice://disconnected";

/**
 * Talks to the Rust media engine. Command names and argument shapes must match
 * `src-tauri/src/commands.rs` (Tauri converts camelCase keys to snake_case params).
 */
export class TauriEngine implements VoiceEngine {
  joinChannel(request: JoinRequest): Promise<void> {
    return invoke("voice_join_channel", {
      campId: request.campId,
      channelId: request.channelId,
      members: request.members,
    });
  }

  leaveChannel(): Promise<void> {
    return invoke("voice_leave_channel");
  }

  setVolume(userId: UserId, percent: number): Promise<void> {
    return invoke("voice_set_volume", { userId, percent });
  }

  setLocalMute(userId: UserId, muted: boolean): Promise<void> {
    return invoke("voice_set_local_mute", { userId, muted });
  }

  setMicMuted(muted: boolean): Promise<void> {
    return invoke("voice_set_mic_muted", { muted });
  }

  setDeafened(deafened: boolean): Promise<void> {
    return invoke("voice_set_deafened", { deafened });
  }

  onLevels(listener: (levels: readonly MemberLevel[]) => void): () => void {
    return subscribe<MemberLevel[]>(LEVELS_EVENT, listener);
  }

  onParticipants(listener: (update: ParticipantsUpdate) => void): () => void {
    return subscribe<ParticipantsUpdate>(PARTICIPANTS_EVENT, listener);
  }

  onDisconnected(listener: (reason: string) => void): () => void {
    return subscribe<string>(DISCONNECTED_EVENT, listener);
  }
}

/** True inside the Tauri webview, false in a regular browser (`pnpm dev`). */
export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}
