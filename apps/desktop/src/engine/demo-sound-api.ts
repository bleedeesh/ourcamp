import type { HotkeyAction } from "./engine.ts";
import type { Hotkey, MicInput, MicMode, SoundApi, SoundSettings, TalkKey } from "./sound-api.ts";

/** Sound settings for looking at the layout in a browser: nothing plays. */
export class DemoSoundApi implements SoundApi {
  private state: SoundSettings = {
    outputs: ["Наушники (USB Audio)", "Динамики (Realtek Audio)", "HDMI-монитор"],
    defaultOutput: "Динамики (Realtek Audio)",
    microphones: ["Микрофон (USB Audio)", "Гарнитура"],
    defaultMicrophone: "Микрофон (USB Audio)",
    output: null,
    microphone: null,
    volume: 100,
    micMode: "voice",
    voiceThreshold: -50,
    talkKey: null,
    muteKey: null,
    deafenKey: null,
    talkKeyWorks: true,
  };

  settings(): Promise<SoundSettings> {
    return Promise.resolve(this.state);
  }

  setOutput(device: string | null): Promise<SoundSettings> {
    this.state = { ...this.state, output: device };
    return this.settings();
  }

  setMicrophone(device: string | null): Promise<SoundSettings> {
    this.state = { ...this.state, microphone: device };
    return this.settings();
  }

  setVolume(percent: number): Promise<void> {
    this.state = { ...this.state, volume: percent };
    return Promise.resolve();
  }

  setMicMode(mode: MicMode): Promise<SoundSettings> {
    this.state = { ...this.state, micMode: mode };
    return this.settings();
  }

  setThreshold(db: number): Promise<void> {
    this.state = { ...this.state, voiceThreshold: db };
    return Promise.resolve();
  }

  setTalkKey(key: TalkKey | null): Promise<SoundSettings> {
    this.state = { ...this.state, talkKey: key };
    return this.settings();
  }

  setHotkey(action: HotkeyAction, key: Hotkey | null): Promise<SoundSettings> {
    this.state = action === "mic" ? { ...this.state, muteKey: key } : { ...this.state, deafenKey: key };
    return this.settings();
  }

  micCheck(): Promise<void> {
    return Promise.resolve();
  }

  /** A voice that rises and falls, to see the bar move. */
  micInput(): Promise<MicInput> {
    const levelDb = -45 + 20 * Math.sin(Date.now() / 400);
    return Promise.resolve({ levelDb, open: levelDb >= this.state.voiceThreshold });
  }

  onChanged(): () => void {
    return () => {};
  }
}
