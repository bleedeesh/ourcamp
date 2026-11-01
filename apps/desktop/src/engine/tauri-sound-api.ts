import { invoke } from "@tauri-apps/api/core";
import { subscribe } from "@ourcamp/ui/tauri-events.ts";
import type { SoundApi, SoundSettings } from "./sound-api.ts";

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

  onChanged(listener: (settings: SoundSettings) => void): () => void {
    return subscribe<SoundSettings>(SOUND_EVENT, listener);
  }
}
