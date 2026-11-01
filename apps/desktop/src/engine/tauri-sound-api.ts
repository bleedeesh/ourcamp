import { invoke } from "@tauri-apps/api/core";
import { subscribe } from "@ourcamp/ui/tauri-events.ts";
import type { HotkeyAction } from "./engine.ts";
import type { Hotkey, MicInput, MicMode, SoundApi, SoundSettings, TalkKey } from "./sound-api.ts";

/** Event emitted by `src-tauri/src/lib.rs` (`SOUND_EVENT`). */
const SOUND_EVENT = "sound://changed";

/** Sound commands of `src-tauri/src/commands.rs`. */
export class TauriSoundApi implements SoundApi {
  settings(): Promise<SoundSettings> {
    return invoke("sound_view");
  }

  setOutput(device: string | null): Promise<SoundSettings> {
    return invoke("sound_set_output", { device });
  }

  setMicrophone(device: string | null): Promise<SoundSettings> {
    return invoke("sound_set_microphone", { device });
  }

  setVolume(percent: number, save: boolean): Promise<void> {
    return invoke("sound_set_volume", { percent, save });
  }

  setMicMode(mode: MicMode): Promise<SoundSettings> {
    return invoke("sound_set_mic_mode", { mode });
  }

  setThreshold(db: number, save: boolean): Promise<void> {
    return invoke("sound_set_threshold", { db, save });
  }

  setTalkKey(key: TalkKey | null): Promise<SoundSettings> {
    return invoke("sound_set_talk_key", { key });
  }

  setHotkey(action: HotkeyAction, key: Hotkey | null): Promise<SoundSettings> {
    return invoke("sound_set_hotkey", { action, key });
  }

  micCheck(on: boolean): Promise<void> {
    return invoke("sound_mic_check", { on });
  }

  micInput(): Promise<MicInput> {
    return invoke("sound_mic_input");
  }

  onChanged(listener: (settings: SoundSettings) => void): () => void {
    return subscribe<SoundSettings>(SOUND_EVENT, listener);
  }
}
