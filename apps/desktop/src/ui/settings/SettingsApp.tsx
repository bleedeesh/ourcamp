import { useEffect, useMemo, useState, type FormEvent } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Icon } from "@ourcamp/ui/Icon.tsx";
import type { IconName } from "@ourcamp/ui/Icon.tsx";
import { canSaveName, MAX_NAME_CHARS, nameProblem } from "../../domain/settings.ts";
import type { CampApi } from "../../engine/camp-api.ts";
import { DemoCampApi } from "../../engine/demo-camp-api.ts";
import { DemoSoundApi } from "../../engine/demo-sound-api.ts";
import type { SoundApi } from "../../engine/sound-api.ts";
import { TauriCampApi } from "../../engine/tauri-camp-api.ts";
import { isTauri } from "../../engine/tauri-engine.ts";
import { TauriSoundApi } from "../../engine/tauri-sound-api.ts";
import { AvatarPicker } from "../AvatarPicker.tsx";
import { KeysTab } from "./KeysTab.tsx";
import { SoundTab } from "./SoundTab.tsx";

type Tab = "sound" | "keys" | "profile";

const TABS: readonly { readonly id: Tab; readonly title: string; readonly icon: IconName }[] = [
  { id: "sound", title: "Звук", icon: "headphones" },
  { id: "keys", title: "Клавиши", icon: "keyboard" },
  { id: "profile", title: "Профиль", icon: "user" },
];

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** Your name and avatar (ADR-0057): kept on this device, sent to the servers. */
function ProfileTab({ api }: { api: CampApi }) {
  const [saved, setSaved] = useState("");
  const [name, setName] = useState("");
  const [avatar, setAvatar] = useState<Uint8Array | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  useEffect(() => {
    void api.list().then((list) => {
      setSaved(list.displayName);
      setName(list.displayName);
    });
    void api.profileAvatar().then(setAvatar);
  }, [api]);

  const changeAvatar = async (picture: Uint8Array | null) => {
    try {
      setProblem(null);
      await api.setProfileAvatar(picture);
      setAvatar(picture);
    } catch (e) {
      setProblem(message(e));
    }
  };

  const save = async (e: FormEvent) => {
    e.preventDefault();
    if (!canSaveName(name, saved)) return;
    try {
      setProblem(null);
      await api.setProfileName(name);
      const trimmed = name.trim();
      setSaved(trimmed);
      setName(trimmed);
    } catch (err) {
      setProblem(message(err));
    }
  };

  const wrong = name === "" && saved === "" ? null : nameProblem(name);
  const shown = wrong ?? problem;

  return (
    <form className="settings-section" onSubmit={(e) => void save(e)}>
      <h2 className="settings-section__title">Профиль</h2>
      <AvatarPicker name={name} avatar={avatar} onChange={changeAvatar} />
      <label className="field__label" htmlFor="settings-name">
        Как вас называть
      </label>
      <div className="settings-name">
        <div className="field settings-name__field">
          <input
            id="settings-name"
            className="field__input"
            value={name}
            maxLength={MAX_NAME_CHARS * 2}
            autoComplete="off"
            onChange={(e) => setName(e.target.value)}
          />
        </div>
        <button type="submit" className="primary-button settings-name__save" disabled={!canSaveName(name, saved)}>
          Сохранить
        </button>
      </div>
      {shown !== null && (
        <p className="settings-section__error" role="alert">
          {shown}
        </p>
      )}
      <p className="settings-section__note">
        Открытый сервер увидит новое имя и аватарку сразу, остальные — когда вы их откроете.
      </p>
    </form>
  );
}

/** Arrives with a later step of M7 (ADR-0057). */
/** The settings window (M7, ADR-0057): the mockup's screen 13. */
export function SettingsApp() {
  const api = useMemo<CampApi>(() => (isTauri() ? new TauriCampApi() : new DemoCampApi()), []);
  const sound = useMemo<SoundApi>(() => (isTauri() ? new TauriSoundApi() : new DemoSoundApi()), []);
  const [tab, setTab] = useState<Tab>("sound");

  const minimize = () => {
    if (isTauri()) void getCurrentWindow().minimize();
  };
  const close = () => {
    if (isTauri()) void getCurrentWindow().close();
    else window.close();
  };

  return (
    <div className="settings-app">
      <header className="titlebar" data-tauri-drag-region>
        <span className="titlebar__brand titlebar__brand--chat" data-tauri-drag-region>
          <Icon name="gear" size={14} />
          <span className="titlebar__title" data-tauri-drag-region>
            Настройки
          </span>
        </span>
        <button type="button" className="titlebar__button" aria-label="Свернуть" onClick={minimize}>
          <Icon name="minimize" size={14} />
        </button>
        <button type="button" className="titlebar__button titlebar__button--close" aria-label="Закрыть окно" onClick={close}>
          <Icon name="close" size={14} />
        </button>
      </header>
      <div className="settings-app__main">
        <nav className="settings-nav" aria-label="Разделы настроек">
          {TABS.map((t) => (
            <button
              key={t.id}
              type="button"
              className="settings-nav__tab"
              aria-current={t.id === tab ? "page" : undefined}
              onClick={() => setTab(t.id)}
            >
              <Icon name={t.icon} size={16} />
              {t.title}
            </button>
          ))}
        </nav>
        <main className="settings-app__body">
          {tab === "profile" && <ProfileTab api={api} />}
          {tab === "sound" && <SoundTab api={sound} />}
          {tab === "keys" && <KeysTab api={sound} />}
        </main>
      </div>
    </div>
  );
}
