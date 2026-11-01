/** How the microphone goes on the air (ADR-0069). */
export type MicMode = "voice" | "key";

/** The key that opens the microphone (`TalkKey` in `src-tauri/src/audio/devices.rs`). */
export interface TalkKey {
  /** Windows virtual-key code. */
  readonly code: number;
  readonly label: string;
}

/** Which event sounds are on (`CuePrefs` in `src-tauri/src/audio/devices.rs`). */
export interface CueSettings {
  readonly micOff: boolean;
  readonly micOn: boolean;
  readonly joined: boolean;
  readonly left: boolean;
  readonly streamStarted: boolean;
}

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
  readonly micMode: MicMode;
  /** Where the voice opens the microphone, dBFS. */
  readonly voiceThreshold: number;
  readonly talkKey: TalkKey | null;
  /** A key can be read on this system (ADR-0069). */
  readonly talkKeyWorks: boolean;
  readonly noiseSuppression: boolean;
  readonly echoCancellation: boolean;
  readonly cues: CueSettings;
}

/** What the microphone hears now. */
export interface MicInput {
  readonly levelDb: number;
  /** The microphone would be on the air at this level. */
  readonly open: boolean;
}

/** What the windows ask of the sound engine (ADR-0066, ADR-0069). */
export interface SoundApi {
  settings(): Promise<SoundSettings>;
  setOutput(device: string | null): Promise<SoundSettings>;
  setMicrophone(device: string | null): Promise<SoundSettings>;
  /** `save`: the hand is off the slider, keep the value. */
  setVolume(percent: number, save: boolean): Promise<void>;
  setMicMode(mode: MicMode): Promise<SoundSettings>;
  /** `save`: the hand is off the slider, keep the value. */
  setThreshold(db: number, save: boolean): Promise<void>;
  setTalkKey(key: TalkKey | null): Promise<SoundSettings>;
  setProcessing(noiseSuppression: boolean, echoCancellation: boolean): Promise<SoundSettings>;
  /** One just turned on plays, to show what it is (ADR-0075). */
  setCues(cues: CueSettings): Promise<SoundSettings>;
  /** The level bar came on screen or left it; outside a channel it opens the microphone. */
  micCheck(on: boolean): Promise<void>;
  micInput(): Promise<MicInput>;
  /** Devices came or went; returns the unsubscribe. */
  onChanged(listener: (settings: SoundSettings) => void): () => void;
}
