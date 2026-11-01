/** The sound settings as Rust has them (`SoundView` in `src-tauri/src/audio/engine.rs`). */
export interface SoundSettings {
  readonly outputs: readonly string[];
  readonly defaultOutput: string | null;
  readonly microphones: readonly string[];
  readonly defaultMicrophone: string | null;
  /** The chosen device; `null` follows the system. */
  readonly output: string | null;
  readonly microphone: string | null;
  /** Overall volume, percent. */
  readonly volume: number;
}

/** What the settings window asks of the sound engine (ADR-0066). */
export interface SoundApi {
  settings(): Promise<SoundSettings>;
  setOutput(device: string | null): Promise<SoundSettings>;
  setMicrophone(device: string | null): Promise<SoundSettings>;
  /** `save`: the hand is off the slider, keep the value. */
  setVolume(percent: number, save: boolean): Promise<void>;
  /** Devices came or went; returns the unsubscribe. */
  onChanged(listener: (settings: SoundSettings) => void): () => void;
}
