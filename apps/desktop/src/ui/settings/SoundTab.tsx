import { useCallback, useEffect, useRef, useState, type CSSProperties } from "react";
import { Select } from "@ourcamp/ui/Select.tsx";
import {
  deviceChoice,
  deviceOptions,
  deviceValue,
  MAX_THRESHOLD_DB,
  meterPercent,
  MIN_THRESHOLD_DB,
  talkKeyFromMouse,
  talkKeyFromPress,
  thresholdLabel,
} from "../../domain/sound.ts";
import type { MicMode, SoundApi, SoundSettings, TalkKey } from "../../engine/sound-api.ts";

/** How often the bar asks for the level: the rate levels go by everywhere (~20 Hz). */
const METER_EVERY_MS = 50;

const VOICE_MODE: { value: MicMode; label: string } = { value: "voice", label: "Голосом" };
const KEY_MODE: { value: MicMode; label: string } = { value: "key", label: "Пока зажата клавиша" };

/**
 * The key that opens the microphone: the next key or side mouse button pressed becomes it,
 * Esc changes nothing. Nothing else about the keyboard is looked at (ADR-0069).
 */
function TalkKeyPicker({ value, onPick }: { value: TalkKey | null; onPick: (key: TalkKey) => void }) {
  const [waiting, setWaiting] = useState(false);

  useEffect(() => {
    if (!waiting) return undefined;
    const pick = (key: TalkKey | null) => {
      if (key === null) return;
      setWaiting(false);
      onPick(key);
    };
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      if (e.code === "Escape") setWaiting(false);
      else pick(talkKeyFromPress(e.code, e.keyCode));
    };
    const onMouse = (e: MouseEvent) => {
      const key = talkKeyFromMouse(e.button);
      if (key !== null) e.preventDefault();
      pick(key);
    };
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("mousedown", onMouse, true);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("mousedown", onMouse, true);
    };
  }, [waiting, onPick]);

  return (
    <button type="button" className="wide-button talk-key" data-waiting={waiting || undefined} onClick={() => setWaiting(!waiting)}>
      {waiting ? "Нажмите клавишу… (Esc — отмена)" : (value?.label ?? "Назначить клавишу")}
    </button>
  );
}

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/**
 * What the microphone hears against the threshold (ADR-0069). The bar is drawn past React:
 * twenty renders a second for one width would be waste. Outside a channel the microphone is
 * open for it only while it is on screen.
 */
function MicMeter({ api, threshold, onProblem }: { api: SoundApi; threshold: number | null; onProblem: (text: string) => void }) {
  const fill = useRef<HTMLDivElement>(null);

  useEffect(() => {
    api.micCheck(true).catch((e: unknown) => onProblem(message(e)));
    const timer = setInterval(() => {
      void api.micInput().then(({ levelDb, open }) => {
        if (fill.current === null) return;
        fill.current.style.width = `${meterPercent(levelDb)}%`;
        fill.current.dataset["open"] = String(open);
      });
    }, METER_EVERY_MS);
    return () => {
      clearInterval(timer);
      void api.micCheck(false);
    };
  }, [api, onProblem]);

  return (
    <div className="mic-meter" aria-hidden="true">
      <div className="mic-meter__fill" ref={fill} />
      {threshold !== null && <div className="mic-meter__mark" style={{ left: `${meterPercent(threshold)}%` }} />}
    </div>
  );
}

function Switch({ name, on, onChange }: { name: string; on: boolean; onChange: (on: boolean) => void }) {
  return (
    <div className="settings-switch">
      <span className="settings-switch__name">{name}</span>
      <button type="button" role="switch" className="settings-switch__track" aria-checked={on} aria-label={name} onClick={() => onChange(!on)}>
        <span className="settings-switch__thumb" />
      </button>
    </div>
  );
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

  const pickKey = useCallback(
    (key: TalkKey) => {
      api.setTalkKey(key).then(setSound, (e: unknown) => setProblem(message(e)));
    },
    [api],
  );

  const byKey = sound?.talkKeyWorks === true && sound.micMode === "key";

  const changeVolume = (volume: number, save: boolean) => {
    setSound((now) => (now === null ? now : { ...now, volume }));
    api.setVolume(volume, save).catch((e: unknown) => setProblem(message(e)));
  };

  const changeThreshold = (voiceThreshold: number, save: boolean) => {
    setSound((now) => (now === null ? now : { ...now, voiceThreshold }));
    api.setThreshold(voiceThreshold, save).catch((e: unknown) => setProblem(message(e)));
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
          <span id="settings-mic-mode" className="field__label">
            Как включается микрофон
          </span>
          <Select
            labelledBy="settings-mic-mode"
            value={sound.micMode}
            options={sound.talkKeyWorks ? [VOICE_MODE, KEY_MODE] : [VOICE_MODE]}
            onChange={(mode) => void choose(api.setMicMode(mode))}
          />
          <MicMeter api={api} threshold={byKey ? null : sound.voiceThreshold} onProblem={setProblem} />
          {!byKey ? (
            <>
              <label className="volume-menu__label" htmlFor="settings-threshold">
                Порог
                <output htmlFor="settings-threshold" className="volume-menu__value">
                  {thresholdLabel(sound.voiceThreshold)}
                </output>
              </label>
              <input
                id="settings-threshold"
                className="volume-slider"
                type="range"
                min={MIN_THRESHOLD_DB}
                max={MAX_THRESHOLD_DB}
                value={sound.voiceThreshold}
                style={{ "--fill": `${meterPercent(sound.voiceThreshold)}%` } as CSSProperties}
                onChange={(e) => changeThreshold(Number(e.target.value), false)}
                onPointerUp={(e) => changeThreshold(Number(e.currentTarget.value), true)}
                onKeyUp={(e) => changeThreshold(Number(e.currentTarget.value), true)}
              />
              <p className="settings-section__note">Голос идёт в канал, когда полоска заходит за отметку.</p>
            </>
          ) : (
            <>
              <span className="field__label">Клавиша</span>
              <TalkKeyPicker value={sound.talkKey} onPick={pickKey} />
              <p className="settings-section__note">
                {sound.talkKey === null
                  ? "Пока клавиша не назначена, микрофон не включится."
                  : "Голос идёт в канал, пока клавиша зажата, даже если окно свёрнуто. В других программах она работает как обычно."}
              </p>
            </>
          )}
          <Switch
            name="Шумоподавление"
            on={sound.noiseSuppression}
            onChange={(on) => void choose(api.setProcessing(on, sound.echoCancellation))}
          />
          <Switch
            name="Эхоподавление"
            on={sound.echoCancellation}
            onChange={(on) => void choose(api.setProcessing(sound.noiseSuppression, on))}
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
