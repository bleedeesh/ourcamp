import { useEffect, useState, type CSSProperties } from "react";
import { Select } from "@ourcamp/ui/Select.tsx";
import { deviceChoice, deviceOptions, deviceValue } from "../../domain/sound.ts";
import type { SoundApi, SoundSettings } from "../../engine/sound-api.ts";

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** Devices and the overall volume (ADR-0066): they change at once, in a channel too. */
export function SoundTab({ api }: { api: SoundApi }) {
  const [sound, setSound] = useState<SoundSettings | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  useEffect(() => {
    void api.settings().then(setSound, (e: unknown) => setProblem(message(e)));
    return api.onChanged(setSound);
  }, [api]);

  const choose = async (change: Promise<SoundSettings>) => {
    try {
      setProblem(null);
      setSound(await change);
    } catch (e) {
      setProblem(message(e));
    }
  };

  const changeVolume = (volume: number, save: boolean) => {
    setSound((now) => (now === null ? now : { ...now, volume }));
    api.setVolume(volume, save).catch((e: unknown) => setProblem(message(e)));
  };

  return (
    <div className="settings-section">
      <h2 className="settings-section__title">Звук</h2>
      {sound !== null && (
        <>
          <span id="settings-mic" className="field__label">
            Микрофон
          </span>
          <Select
            labelledBy="settings-mic"
            value={deviceValue(sound.microphone)}
            options={deviceOptions(sound.microphones, sound.defaultMicrophone, sound.microphone)}
            onChange={(value) => void choose(api.setMicrophone(deviceChoice(value)))}
          />
          <span id="settings-output" className="field__label">
            Наушники или колонки
          </span>
          <Select
            labelledBy="settings-output"
            value={deviceValue(sound.output)}
            options={deviceOptions(sound.outputs, sound.defaultOutput, sound.output)}
            onChange={(value) => void choose(api.setOutput(deviceChoice(value)))}
          />
          <label className="volume-menu__label" htmlFor="settings-volume">
            Общая громкость
            <output htmlFor="settings-volume" className="volume-menu__value">
              {sound.volume}%
            </output>
          </label>
          <input
            id="settings-volume"
            className="volume-slider"
            type="range"
            min={0}
            max={100}
            value={sound.volume}
            style={{ "--fill": `${sound.volume}%` } as CSSProperties}
            onChange={(e) => changeVolume(Number(e.target.value), false)}
            onPointerUp={(e) => changeVolume(Number(e.currentTarget.value), true)}
            onKeyUp={(e) => changeVolume(Number(e.currentTarget.value), true)}
          />
        </>
      )}
      {problem !== null && (
        <p className="settings-section__error" role="alert">
          {problem}
        </p>
      )}
    </div>
  );
}
