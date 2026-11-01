import { useEffect, useState } from "react";
import type { CueSettings, SoundApi, SoundSettings } from "../../engine/sound-api.ts";
import { Switch } from "./SoundTab.tsx";

const CUES: readonly { readonly id: keyof CueSettings; readonly name: string }[] = [
  { id: "micOff", name: "Микрофон выключен" },
  { id: "micOn", name: "Микрофон включён" },
  { id: "joined", name: "Вход в канал" },
  { id: "left", name: "Выход из канала" },
  { id: "streamStarted", name: "Начало стрима" },
];

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** Which event sounds play (ADR-0075). Turning one on lets you hear it. */
export function NotificationsTab({ api }: { api: SoundApi }) {
  const [sound, setSound] = useState<SoundSettings | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  useEffect(() => {
    void api.settings().then(setSound, (e: unknown) => setProblem(message(e)));
    return api.onChanged(setSound);
  }, [api]);

  const change = (cues: CueSettings) => {
    setProblem(null);
    api.setCues(cues).then(setSound, (e: unknown) => setProblem(message(e)));
  };

  return (
    <section className="settings-section">
      <h2 className="settings-section__title">Уведомления</h2>
      {sound &&
        CUES.map((cue) => (
          <Switch
            key={cue.id}
            name={cue.name}
            on={sound.cues[cue.id]}
            onChange={(on) => change({ ...sound.cues, [cue.id]: on })}
          />
        ))}
      {problem !== null && (
        <p className="settings-section__error" role="alert">
          {problem}
        </p>
      )}
      <p className="settings-section__note">Включённый звук сразу прозвучит. Пока звук приложения выключен, сигналов тоже нет.</p>
    </section>
  );
}
