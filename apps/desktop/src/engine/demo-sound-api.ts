import type { SoundApi, SoundSettings } from "./sound-api.ts";

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

  onChanged(): () => void {
    return () => {};
  }
}
