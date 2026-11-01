import { useCallback, useEffect, useState } from "react";
import { hotkeyFromMouse, hotkeyFromPress, plainKey, sameKey } from "../../domain/sound.ts";
import type { HotkeyAction } from "../../engine/engine.ts";
import type { Hotkey, SoundApi, SoundSettings } from "../../engine/sound-api.ts";
import { KeyPicker } from "./KeyPicker.tsx";

const ROWS: readonly { action: HotkeyAction; title: string }[] = [
  { action: "mic", title: "Выключить и включить микрофон" },
  { action: "sound", title: "Выключить и включить звук" },
];

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

const fromKey = (e: KeyboardEvent) => hotkeyFromPress(e.code, e.keyCode, { ctrl: e.ctrlKey, shift: e.shiftKey, alt: e.altKey });
const fromMouse = (e: MouseEvent) => hotkeyFromMouse(e.button, { ctrl: e.ctrlKey, shift: e.shiftKey, alt: e.altKey });

function keyOf(sound: SoundSettings, action: HotkeyAction): Hotkey | null {
  return action === "mic" ? sound.muteKey : sound.deafenKey;
}

/** The keys that work whichever window has the focus (ADR-0071). */
export function KeysTab({ api }: { api: SoundApi }) {
  const [sound, setSound] = useState<SoundSettings | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  useEffect(() => {
    void api.settings().then(setSound, (e: unknown) => setProblem(message(e)));
    return api.onChanged(setSound);
  }, [api]);

  const assign = useCallback(
    (action: HotkeyAction, key: Hotkey | null) => {
      setProblem(null);
      api.setHotkey(action, key).then(setSound, (e: unknown) => setProblem(message(e)));
    },
    [api],
  );

  const pick = (action: HotkeyAction, key: Hotkey) => {
    if (sound === null) return;
    const other = keyOf(sound, action === "mic" ? "sound" : "mic");
    if (sameKey(key, other) || sameKey(key, plainKey(sound.talkKey))) setProblem(`${key.label} уже занята`);
    else assign(action, key);
  };

  return (
    <div className="settings-section">
      <h2 className="settings-section__title">Клавиши</h2>
      {sound !== null &&
        (sound.talkKeyWorks ? (
          <>
            {ROWS.map(({ action, title }) => (
              <div key={action} className="settings-key">
                <span className="field__label">{title}</span>
                <div className="settings-key__row">
                  <KeyPicker
                    value={keyOf(sound, action)}
                    empty="Назначить"
                    fromKey={fromKey}
                    fromMouse={fromMouse}
                    onPick={(key) => pick(action, key)}
                  />
                  {keyOf(sound, action) !== null && (
                    <button type="button" className="wide-button talk-key" onClick={() => assign(action, null)}>
                      Убрать
                    </button>
                  )}
                </div>
              </div>
            ))}
            <p className="settings-section__note">
              Работают, даже если окно свёрнуто. Подойдут F-клавиши, боковые кнопки мыши и сочетания с Ctrl, Shift или Alt: одна буква мешала бы печатать.
            </p>
          </>
        ) : (
          <p className="settings-section__note">На этой системе клавиши пока не работают.</p>
        ))}
      {problem !== null && (
        <p className="settings-section__error" role="alert">
          {problem}
        </p>
      )}
    </div>
  );
}
