import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Icon } from "@ourcamp/ui/Icon.tsx";
import type { CampApi } from "../../engine/camp-api.ts";
import { DemoCampApi } from "../../engine/demo-camp-api.ts";
import { TauriCampApi } from "../../engine/tauri-camp-api.ts";
import { isTauri } from "../../engine/tauri-engine.ts";
import {
  bitrateOf,
  channelNow,
  columnOf,
  EMPTY_CHANNEL,
  heavyText,
  soundHint,
  keepChoice,
  sourceTiles,
  uploadText,
  type ChannelNow,
  type QualityOption,
  type StreamSource,
} from "../../domain/stream-start.ts";

/** The demo in a browser captures nothing: for the layout only. */
const DEMO_SOURCES: StreamSource[] = [
  { kind: "screen", id: 0, title: "" },
  { kind: "screen", id: 1, title: "" },
  { kind: "window", id: 2, title: "Игра" },
  { kind: "window", id: 3, title: "Браузер" },
];
const DEMO_QUALITIES: QualityOption[] = [720, 1080, 1440].flatMap((height) =>
  [30, 60].map((fps) => ({ height, fps, bitrate: 6_000_000 })),
);

const api: CampApi = isTauri() ? new TauriCampApi() : new DemoCampApi();

function closeWindow() {
  if (isTauri()) void getCurrentWindow().close();
  else window.close();
}

/** What is on air in your channel, kept up by the camp's snapshots. */
function useChannelNow(): ChannelNow {
  const [now, setNow] = useState(EMPTY_CHANNEL);
  useEffect(() => {
    let selfId: string | null = null;
    let alive = true;
    const stop = api.onLive((live) => {
      if (selfId !== null) setNow(channelNow(live.state, selfId));
    });
    void api.chatContext().then((context) => {
      if (!alive || !context) return;
      selfId = context.camp.self.userId;
      setNow(channelNow(context.live?.state ?? null, selfId));
    });
    return () => {
      alive = false;
      stop();
    };
  }, []);
  return now;
}

/** The list of screens and windows: read on opening and each time you come back to the window. */
function useSources(): readonly StreamSource[] | null {
  const [sources, setSources] = useState<readonly StreamSource[] | null>(isTauri() ? null : DEMO_SOURCES);
  useEffect(() => {
    if (!isTauri()) return undefined;
    const read = () => {
      void invoke<StreamSource[]>("stream_sources").then(setSources);
    };
    read();
    window.addEventListener("focus", read);
    return () => window.removeEventListener("focus", read);
  }, []);
  return sources;
}

function Choice({
  label,
  values,
  value,
  text,
  onChange,
}: {
  label: string;
  values: readonly number[];
  value: number;
  text: (v: number) => string;
  onChange: (v: number) => void;
}) {
  return (
    <div className="start-choice" role="group" aria-label={label}>
      <span className="start-choice__label">{label}</span>
      <div className="start-choice__options">
        {values.map((v) => (
          <button key={v} type="button" className="start-choice__option" aria-pressed={v === value} onClick={() => onChange(v)}>
            {text(v)}
          </button>
        ))}
      </div>
    </div>
  );
}

/** The window "Начать стрим" (M6 step 4, ADR-0054): the mockup's screen 11. */
export function StreamStartApp() {
  const sources = useSources();
  const channel = useChannelNow();
  const [qualities, setQualities] = useState<readonly QualityOption[]>(isTauri() ? [] : DEMO_QUALITIES);
  const [chosen, setChosen] = useState<string | null>(null);
  const [height, setHeight] = useState(1080);
  const [fps, setFps] = useState(60);
  const [sound, setSound] = useState(true);
  const [soundWorks, setSoundWorks] = useState(true);
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!isTauri()) return;
    void invoke<QualityOption[]>("stream_qualities").then(setQualities);
    void invoke<boolean>("stream_sound_available").then(setSoundWorks);
  }, []);

  const tiles = useMemo(() => sourceTiles(sources ?? []), [sources]);
  const selected = keepChoice(tiles, chosen);
  const bitrate = bitrateOf(qualities, height, fps);
  const heavy = heavyText(height);
  const pickedKind = tiles.find((t) => t.key === selected)?.source.kind ?? "screen";

  const start = async () => {
    const tile = tiles.find((t) => t.key === selected);
    if (!tile || starting) return;
    setStarting(true);
    setError(null);
    try {
      if (isTauri()) {
        await invoke("stream_start", {
          kind: tile.source.kind,
          id: tile.source.id,
          quality: { height, fps },
          sound: sound && soundWorks,
        });
      }
      closeWindow();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setStarting(false);
    }
  };

  const watch = async (userId: string, name: string) => {
    try {
      if (isTauri()) await invoke("stream_watch", { userId, name });
      closeWindow();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const minimize = () => {
    if (isTauri()) void getCurrentWindow().minimize();
  };

  return (
    <div className="start-app">
      <header className="titlebar" data-tauri-drag-region>
        <span className="titlebar__brand titlebar__brand--chat" data-tauri-drag-region>
          <Icon name="screen" size={14} />
          <span className="titlebar__title" data-tauri-drag-region>
            Начать стрим
          </span>
        </span>
        <button type="button" className="titlebar__button" aria-label="Свернуть" onClick={minimize}>
          <Icon name="minimize" size={14} />
        </button>
        <button type="button" className="titlebar__button titlebar__button--close" aria-label="Закрыть окно" onClick={closeWindow}>
          <Icon name="close" size={14} />
        </button>
      </header>

      <div className="start-app__body">
        {channel.onAir.map((person) => (
          <div key={person.userId} className="start-live">
            <span className="live-badge">LIVE</span>
            <span className="start-live__who">
              Сейчас в эфире: <b>{person.name}</b>
            </span>
            <button type="button" className="start-live__watch" onClick={() => void watch(person.userId, person.name)}>
              Смотреть
            </button>
          </div>
        ))}

        <h2 className="start-app__heading">Что показать</h2>
        {sources !== null && tiles.length === 0 && <p className="start-app__note">Нет экранов и окон, которые можно показать.</p>}
        <div className="start-sources">
          {tiles.map((tile) => (
            <button
              key={tile.key}
              type="button"
              className="start-source"
              aria-pressed={tile.key === selected}
              title={tile.label}
              onClick={() => setChosen(tile.key)}
            >
              <Icon name="screen" size={18} />
              <span className="start-source__label">{tile.label}</span>
            </button>
          ))}
        </div>

        <div className="start-choices">
          <Choice label="Качество" values={columnOf(qualities, "height")} value={height} text={(v) => `${v}p`} onChange={setHeight} />
          <Choice label="Частота" values={columnOf(qualities, "fps")} value={fps} text={(v) => `${v} fps`} onChange={setFps} />
        </div>

        {bitrate > 0 && <p className="start-app__note">{uploadText(bitrate, channel.others)}</p>}
        {heavy !== null && <p className="start-app__note">{heavy}</p>}

        <label className="start-sound" data-off={!soundWorks || undefined} title={soundWorks ? soundHint(pickedKind) : "Нужна Windows 10 версии 2004 или новее"}>
          <input type="checkbox" checked={sound && soundWorks} disabled={!soundWorks} onChange={(e) => setSound(e.target.checked)} />
          Транслировать звук
        </label>

        {error !== null && (
          <p className="start-app__error" role="alert">
            {error}
          </p>
        )}
      </div>

      <footer className="start-app__actions">
        <button type="button" className="wide-button start-app__cancel" onClick={closeWindow}>
          Отмена
        </button>
        <button
          type="button"
          className="wide-button start-app__go"
          aria-disabled={selected === null || starting || undefined}
          onClick={() => void start()}
        >
          {starting ? "Начинаем…" : "Начать стрим"}
        </button>
      </footer>
    </div>
  );
}
