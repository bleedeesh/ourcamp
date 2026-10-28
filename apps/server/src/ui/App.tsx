import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Campfire } from "@ourcamp/ui/Campfire.tsx";
import { Icon } from "@ourcamp/ui/Icon.tsx";
import { copyText } from "@ourcamp/ui/clipboard.ts";

import { isTauri, type ServerApi } from "../api.ts";
import {
  crowd,
  headline,
  ROLE_LABEL,
  STARTING,
  voice,
  type Channel,
  type Member,
  type Role,
  type ServerStatus,
} from "../status.ts";
import { CampName, Channels, useAction } from "./CampSettings.tsx";

function TitleBar() {
  const win = () => (isTauri() ? getCurrentWindow() : null);
  return (
    <header className="titlebar" data-tauri-drag-region>
      <span className="titlebar__brand" data-tauri-drag-region>
        <Campfire size={14} simple />
        <span className="titlebar__name" data-tauri-drag-region>
          OurCamp Server
        </span>
      </span>
      <button type="button" className="titlebar__button" aria-label="Свернуть" onClick={() => void win()?.minimize()}>
        <Icon name="minimize" size={14} />
      </button>
      <button
        type="button"
        className="titlebar__button titlebar__button--close"
        aria-label="Закрыть"
        onClick={() => void win()?.close()}
      >
        <Icon name="close" size={14} />
      </button>
    </header>
  );
}

function LinkRow({ label, hint, link }: { label: string; hint: string; link: string }) {
  const [state, setState] = useState<"idle" | "copied" | string>("idle");
  const copy = () => {
    copyText(link).then(
      () => setState("copied"),
      (e: unknown) => setState(e instanceof Error ? e.message : String(e)),
    );
  };
  useEffect(() => {
    if (state !== "copied") return;
    const timer = setTimeout(() => setState("idle"), 1500);
    return () => clearTimeout(timer);
  }, [state]);
  return (
    <div className="link">
      <div className="link__head">
        <span className="link__label">{label}</span>
        <button type="button" className="link__copy" onClick={copy}>
          {state === "copied" ? "Скопировано" : "Скопировать"}
        </button>
      </div>
      <code className="link__value">{link}</code>
      <span className="link__hint">{state !== "idle" && state !== "copied" ? state : hint}</span>
    </div>
  );
}

/** A member and the owner's hand on them: role (ADR-0026), move, silence, kick (ADR-0028). */
function MemberRow({ member, channels, api }: { member: Member; channels: readonly Channel[]; api: ServerApi }) {
  const [mode, setMode] = useState<"view" | "move" | "kick">("view");
  const [error, run, clear] = useAction();
  const back = () => {
    clear();
    setMode("view");
  };
  const act = async (action: () => Promise<void>) => {
    if (await run(action)) setMode("view");
  };

  if (mode === "kick") {
    return (
      <li className="member member--confirm" role="alertdialog" aria-label={`Выгнать ${member.name}`}>
        <span className="member__question">
          Выгнать «{member.name}» с сервера? Вернуться можно по ссылке для друзей.
        </span>
        <span className="member__actions">
          <button type="button" className="small-button small-button--danger" onClick={() => void act(() => api.kick(member.userId))}>
            Выгнать
          </button>
          <button type="button" className="small-button" onClick={back}>
            Отмена
          </button>
        </span>
        {error && <span className="member__error">{error}</span>}
      </li>
    );
  }

  const targets = channels.filter((c) => c.kind === "voice" && c.id !== member.campfireId);
  return (
    <li className="member">
      <span className="member__avatar" aria-hidden="true">
        {member.name.slice(0, 1).toUpperCase()}
      </span>
      <span className="member__name">{member.name}</span>
      {member.role === "admin" && (
        <span className="member__crown" title="Админ">
          <Icon name="crown" size={14} />
        </span>
      )}
      {member.role === "moderator" && (
        <span className="member__shield" title="Модератор">
          <Icon name="shield" size={14} />
        </span>
      )}
      {member.silenced ? (
        <span className="member__silenced" title="Говорить запрещено">
          <Icon name="mic" off size={14} />
        </span>
      ) : (
        member.campfire !== null &&
        member.micMuted && (
          <span className="member__muted" title="Микрофон выключен">
            <Icon name="mic" off size={14} />
          </span>
        )
      )}
      <span className={member.campfire === null ? "member__where" : "member__where member__where--seated"}>
        {member.campfire ?? "не в голосе"}
      </span>
      <select
        className="member__role"
        aria-label={`Роль: ${member.name}`}
        value={member.role}
        onChange={(e) => void run(() => api.setRole(member.userId, e.target.value as Role))}
      >
        {(["member", "moderator", "admin"] as const).map((role) => (
          <option key={role} value={role}>
            {ROLE_LABEL[role]}
          </option>
        ))}
      </select>
      <span className="member__tools">
        {member.campfireId !== null && targets.length > 0 && (
          <button
            type="button"
            className="icon-button"
            aria-label={`Перенести ${member.name} в другой канал`}
            title="Перенести в другой канал"
            aria-expanded={mode === "move"}
            onClick={() => (mode === "move" ? back() : setMode("move"))}
          >
            <Icon name="move" size={14} />
          </button>
        )}
        <button
          type="button"
          className="icon-button"
          data-on={member.silenced || undefined}
          aria-pressed={member.silenced}
          aria-label={member.silenced ? `Разрешить ${member.name} говорить` : `Запретить ${member.name} говорить`}
          title={member.silenced ? "Разрешить говорить" : "Запретить говорить"}
          onClick={() => void run(() => api.setSilenced(member.userId, !member.silenced))}
        >
          <Icon name="mic" off={!member.silenced} size={14} />
        </button>
        <button
          type="button"
          className="icon-button icon-button--danger"
          aria-label={`Выгнать ${member.name}`}
          title="Выгнать с сервера"
          onClick={() => {
            clear();
            setMode("kick");
          }}
        >
          <Icon name="kick" size={14} />
        </button>
      </span>
      {mode === "move" && (
        <span className="member__move" role="group" aria-label={`Куда перенести ${member.name}`}>
          <span className="member__move-label">Перенести в</span>
          {targets.map((c) => (
            <button key={c.id} type="button" className="small-button" onClick={() => void act(() => api.moveMember(member.userId, c.id))}>
              {c.name}
            </button>
          ))}
        </span>
      )}
      {error && <span className="member__error">{error}</span>}
    </li>
  );
}

export function App({ api }: { api: ServerApi }) {
  const [status, setStatus] = useState<ServerStatus>(STARTING);
  useEffect(() => {
    const off = api.onStatus(setStatus);
    void api.status().then(setStatus);
    return off;
  }, [api]);
  const { phase } = status;

  return (
    <div className="app">
      <TitleBar />
      <main className="app__body">
        <section className={`status status--${phase.kind}`}>
          <Campfire size={44} />
          <div className="status__text">
            {phase.kind === "running" ? (
              <CampName name={status.campName} api={api} />
            ) : (
              <h1 className="status__title">OurCamp Server</h1>
            )}
            <span className="status__line">
              <span className="status__dot" aria-hidden="true" />
              {headline(status)}
            </span>
          </div>
        </section>

        {phase.kind === "failed" && (
          <section className="failure">
            <pre className="failure__message">{phase.message}</pre>
            <button type="button" className="button button--accent" onClick={() => void api.retry()}>
              Попробовать снова
            </button>
          </section>
        )}

        {phase.kind === "running" && (
          <>
            <section className="links">
              {status.joinLink !== null && (
                <LinkRow label="Для друзей" hint="Отправьте её тем, кого зовёте на сервер." link={status.joinLink} />
              )}
            </section>

            <section className="members">
              <h2 className="members__title">{crowd(status.members)}</h2>
              <ul className="members__list">
                {status.members.map((m) => (
                  <MemberRow key={m.userId} member={m} channels={status.channels} api={api} />
                ))}
              </ul>
            </section>

            <Channels channels={status.channels} api={api} />
          </>
        )}

        <footer className="footer">
          <span>{voice(status)}</span>
          {status.address !== null && <span>Адрес: {status.address}</span>}
          {status.fingerprint !== null && <span className="footer__fp">Отпечаток: {status.fingerprint}</span>}
        </footer>
      </main>
    </div>
  );
}
