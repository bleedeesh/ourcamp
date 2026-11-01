import { useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Icon } from "@ourcamp/ui/Icon.tsx";
import { subscribe } from "@ourcamp/ui/tauri-events.ts";
import {
  MAX_STREAM_VOLUME,
  physicalRect,
  qualityText,
  readStreamVolume,
  saveStreamVolume,
  streamerOf,
  viewersText,
  waitingText,
  type StreamView,
  type ViewStatus,
} from "../../domain/stream-view.ts";
import { VOLUME_STEP } from "../../domain/volume.ts";
import type { CampApi, CampState } from "../../engine/camp-api.ts";
import { DemoCampApi } from "../../engine/demo-camp-api.ts";
import { TauriCampApi } from "../../engine/tauri-camp-api.ts";
import { isTauri } from "../../engine/tauri-engine.ts";
import { Avatar } from "../Avatar.tsx";
import { AvatarSourceContext, type AvatarSource } from "../avatar-url.ts";

/** Must match `VIEW_EVENT` in `src-tauri/src/stream/view/mod.rs`. */
const STREAM_VIEW_EVENT = "stream://view";
/** Must match `INPUT_EVENT` there. */
const STREAM_INPUT_EVENT = "stream://input";
/** Must match `READY_EVENT` there. */
const STREAM_READY_EVENT = "stream://ready";

const NOTHING_YET: ViewStatus = { live: false, height: 0, fps: 0, viewers: 1, sound: false, reduced: false };
/** The demo in a browser has no stream: for the layout only. */
const DEMO_VIEW: StreamView = {
  userId: "lena",
  name: "Лена",
  status: { live: true, height: 1080, fps: 60, viewers: 3, sound: true, reduced: false },
};

const api: CampApi = isTauri() ? new TauriCampApi() : new DemoCampApi();

/** The open camp and its latest snapshot, kept up by the camp's events. */
function useCampNow(): { readonly id: string | null; readonly state: CampState | null } {
  const [now, setNow] = useState<{ id: string | null; state: CampState | null }>({ id: null, state: null });
  useEffect(() => {
    let alive = true;
    const stop = api.onLive((live) => setNow({ id: live.campId, state: live.state }));
    void api.chatContext().then((context) => {
      if (alive && context) setNow({ id: context.camp.id, state: context.live?.state ?? null });
    });
    return () => {
      alive = false;
      stop();
    };
  }, []);
  return now;
}

/** The viewer's window (M6 step 3, ADR-0053): the mockup's screen 12. */
export function StreamApp() {
  const [view, setView] = useState<StreamView | null>(isTauri() ? null : DEMO_VIEW);
  const camp = useCampNow();
  const campId = camp.id;
  const avatars = useMemo<AvatarSource | null>(
    () => (campId === null ? null : (userId, hash) => api.avatar(campId, userId, hash)),
    [campId],
  );
  const [seen, setSeen] = useState(false);
  const [fullscreen, setFullscreen] = useState(false);
  const [volume, setVolume] = useState(() => readStreamVolume(window.localStorage));
  // For the listeners, set up once: they need the value of the moment.
  const fullscreenNow = useRef(false);
  const picture = useRef<HTMLDivElement>(null);

  const place = () => {
    const element = picture.current;
    if (!element || !isTauri()) return;
    const rect = physicalRect(element.getBoundingClientRect(), window.devicePixelRatio);
    void invoke("stream_place", rect);
  };

  // Keeps the window's state and the page's in step: Esc and a double click on the
  // picture come from Rust, the button from here.
  const setWindowFullscreen = (on: boolean) => {
    fullscreenNow.current = on;
    setFullscreen(on);
    if (isTauri()) void getCurrentWindow().setFullscreen(on);
  };

  useEffect(() => {
    if (!isTauri()) return undefined;
    // On opening and on "ready": the page can load before the viewer is up, or after.
    const sync = () => {
      void invoke<StreamView | null>("stream_view").then((now) => {
        if (now) setView(now);
        place();
      });
    };
    const stopReady = subscribe<null>(STREAM_READY_EVENT, sync);
    const stopView = subscribe<ViewStatus>(STREAM_VIEW_EVENT, (status) => {
      setView((v) => (v ? { ...v, status } : v));
    });
    sync();
    return () => {
      stopReady();
      stopView();
    };
  }, []);

  // Rust plays the sound and holds its volume; this page remembers it between sessions.
  useEffect(() => {
    saveStreamVolume(window.localStorage, volume);
    if (isTauri()) void invoke("stream_volume", { percent: volume });
  }, [volume]);

  useEffect(() => {
    const onInput = (input: string) => {
      if (input === "doubleClick") setWindowFullscreen(!fullscreenNow.current);
      if (input === "escape" && fullscreenNow.current) setWindowFullscreen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onInput("escape");
    };
    window.addEventListener("keydown", onKey);
    const stopInput = isTauri() ? subscribe<string>(STREAM_INPUT_EVENT, onInput) : undefined;
    return () => {
      window.removeEventListener("keydown", onKey);
      stopInput?.();
    };
  }, []);

  useEffect(() => {
    const element = picture.current;
    if (!element) return undefined;
    const observer = new ResizeObserver(place);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  const status = view?.status ?? NOTHING_YET;
  useEffect(() => {
    if (status.live) setSeen(true);
  }, [status.live]);

  const minimize = () => {
    if (isTauri()) void getCurrentWindow().minimize();
  };
  // Closes only the viewing: the voice stays (ADR-0049).
  const close = () => {
    if (isTauri()) void getCurrentWindow().close();
    else window.close();
  };
  const streamer = streamerOf(camp.state, view?.userId ?? "", view?.name ?? "?");
  const title = view ? `Стрим · ${streamer.name}` : "Стрим";
  const quality = qualityText(status);
  // A full screen window has nowhere to be dragged to.
  const drag = fullscreen ? {} : { "data-tauri-drag-region": true };

  return (
    <div className="stream-app" data-fullscreen={fullscreen || undefined}>
      <header className="titlebar" {...drag}>
        <span className="titlebar__brand titlebar__brand--chat" {...drag}>
          <Icon name="screen" size={14} />
          <span className="titlebar__title" {...drag}>
            {title}
          </span>
        </span>
        <button type="button" className="titlebar__button" aria-label="Свернуть" onClick={minimize}>
          <Icon name="minimize" size={14} />
        </button>
        <button type="button" className="titlebar__button titlebar__button--close" aria-label="Закрыть окно" onClick={close}>
          <Icon name="close" size={14} />
        </button>
      </header>
      <div className="stream-app__picture" ref={picture} data-demo={!isTauri() || undefined}>
        {!status.live && <p className="stream-app__waiting">{waitingText(status, seen)}</p>}
      </div>
      <footer className="stream-panel">
        <AvatarSourceContext.Provider value={avatars}>
          <Avatar member={streamer} size={fullscreen ? 22 : 32} speaking={false} />
        </AvatarSourceContext.Provider>
        <span className="stream-panel__who">
          <span className="stream-panel__name">{view ? streamer.name : ""}</span>
          <span className="stream-panel__quality">{quality}</span>
        </span>
        {status.live && <span className="live-badge">LIVE</span>}
        <span className="stream-panel__viewers">
          <Icon name="users" size={14} />
          {viewersText(status.viewers)}
        </span>
        <span className="stream-panel__spacer" />
        <span className="stream-panel__volume" data-off={!status.sound || undefined} title={status.sound ? undefined : "Этот стрим идёт без звука"}>
          <Icon name="speaker" size={16} />
          <input
            className="volume-slider"
            type="range"
            min="0"
            max={MAX_STREAM_VOLUME}
            step={VOLUME_STEP}
            value={volume}
            aria-label="Громкость стрима"
            disabled={!status.sound}
            style={{ "--fill": `${(volume / MAX_STREAM_VOLUME) * 100}%` } as CSSProperties}
            onChange={(e) => setVolume(Number(e.target.value))}
          />
          <span className="stream-panel__percent">{volume} %</span>
        </span>
        <button
          type="button"
          className="stream-panel__button"
          aria-label={fullscreen ? "Выйти из полноэкранного режима" : "Во весь экран"}
          onClick={() => setWindowFullscreen(!fullscreen)}
        >
          <Icon name={fullscreen ? "fullscreenExit" : "fullscreen"} size={fullscreen ? 16 : 18} />
        </button>
      </footer>
    </div>
  );
}
