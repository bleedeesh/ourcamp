import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { UserId } from "../domain/types.ts";
import type { JoinRequest, MemberLevel, VoiceEngine } from "./engine.ts";

/** Event emitted by `src-tauri/src/levels.rs`. */
const LEVELS_EVENT = "voice://levels";

/**
 * Talks to the Rust media engine. Command names and argument shapes must match
 * `src-tauri/src/commands.rs` (Tauri converts camelCase keys to snake_case params).
 */
export class TauriEngine implements VoiceEngine {
  joinChannel(request: JoinRequest): Promise<void> {
    return invoke("voice_join_channel", { members: request.members });
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
    let disposed = false;
    let unlisten: (() => void) | null = null;
    void listen<MemberLevel[]>(LEVELS_EVENT, (event) => listener(event.payload)).then((fn) => {
      // The subscription resolves asynchronously; honour an unsubscribe that came first.
      if (disposed) fn();
      else unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }
}

/** True inside the Tauri webview, false in a regular browser (`pnpm dev`). */
export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}
