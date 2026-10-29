import { useCallback, useEffect, useState, type KeyboardEvent, type MouseEvent, type PointerEvent } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Campfire } from "@ourcamp/ui/Campfire.tsx";
import { Icon } from "@ourcamp/ui/Icon.tsx";
import { copyText } from "@ourcamp/ui/clipboard.ts";
import { DragGhost, useDragMove } from "@ourcamp/ui/useDragMove.tsx";

import { isTauri, type ServerApi } from "../api.ts";
import {
  crowd,
  headline,
  ROLE_LABEL,
  STARTING,
  voice,
  type Member,
  type Role,
  type ServerStatus,
} from "../status.ts";
import { CampName, Channels, useAction, type Dragging } from "./CampSettings.tsx";
import { MemberAvatar } from "./MemberAvatar.tsx";
import { MemberMenu, type MenuAt } from "./MemberMenu.tsx";

function TitleBar() {
  const win = () => (isTauri() ? getCurrentWindow() : null);
  return (
    <header className="titlebar" data-tauri-drag-region>
      <span className="titlebar__brand" data-tauri-drag-region>
        <Campfire size={14} simple server />
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

/**
 * A member and the owner's hand on them: role (ADR-0026), silence and kick (ADR-0028).
 * Someone in voice can be dragged onto another channel (ADR-0029).
 */
function MemberRow({ member, api, drag, grab, openMenu }: { member: Member; api: ServerApi } & Dragging) {
  const [kicking, setKicking] = useState(false);
  const [error, run, clear] = useAction();

  if (kicking) {
    return (
      <li className="member member--confirm" role="alertdialog" aria-label={`Выгнать ${member.name}`}>
        <span className="member__question">Выгнать «{member.name}» с сервера? Вернуться можно по ссылке для друзей.</span>
        <span className="member__actions">
          <button
            type="button"
            className="small-button small-button--danger"
            onClick={() => void run(() => api.kick(member.userId)).then((ok) => ok && setKicking(false))}
          >
            Выгнать
          </button>
          <button
            type="button"
            className="small-button"
            onClick={() => {
              clear();
              setKicking(false);
            }}
          >
            Отмена
          </button>
        </span>
        {error && <span className="member__error">{error}</span>}
      </li>
    );
  }

  const seated = member.campfireId;
  // Only the person is the handle: the controls on the right stay plain clicks.
  const handle =
    seated === null
      ? {}
      : {
          "data-draggable": true,
          onPointerDown: (e: PointerEvent<HTMLElement>) => {
            if ((e.target as HTMLElement).closest("button, select")) return;
            grab({ userId: member.userId, name: member.name, from: seated })(e);
          },
        };
  return (
    <li
      className="member"
      data-dragging={drag?.userId === member.userId || undefined}
      onContextMenu={openMenu(member.userId)}
      {...handle}
    >
      <MemberAvatar member={member} size={32} api={api} />
      <span className="member__who">
        <span className="member__line">
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
              <Icon name="mic" off size={12} />
            </span>
          ) : (
            member.campfire !== null &&
            member.micMuted && (
              <span className="member__muted" title="Микрофон выключен">
                <Icon name="mic" off size={14} />
              </span>
            )
          )}
        </span>
        <span className={member.campfire === null ? "member__where" : "member__where member__where--seated"}>
          {member.campfire === null ? "не в голосе" : `в канале «${member.campfire}»`}
        </span>
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
        <button
          type="button"
          className="icon-button"
          data-on={member.silenced || undefined}
          aria-pressed={member.silenced}
          aria-label={member.silenced ? `Разрешить ${member.name} говорить` : `Запретить ${member.name} говорить`}
          title={member.silenced ? "Разрешить говорить" : "Запретить говорить"}
          onClick={() => void run(() => api.setSilenced(member.userId, !member.silenced))}
        >
          <Icon name="mic" off={!member.silenced} size={16} />
        </button>
        <button
          type="button"
          className="icon-button icon-button--danger"
          aria-label={`Выгнать ${member.name}`}
          title="Выгнать с сервера"
          onClick={() => {
            clear();
            setKicking(true);
          }}
        >
          <Icon name="kick" size={16} />
        </button>
      </span>
      {error && <span className="member__error">{error}</span>}
    </li>
  );
}

export function App({ api }: { api: ServerApi }) {
  const [status, setStatus] = useState<ServerStatus>(STARTING);
  const [moveError, setMoveError] = useState<string | null>(null);
  useEffect(() => {
    const off = api.onStatus(setStatus);
    void api.status().then(setStatus);
    return off;
  }, [api]);
  const { drag, grab } = useDragMove((userId, channelId) => {
    setMoveError(null);
    api.moveMember(userId, channelId).catch((e: unknown) => setMoveError(e instanceof Error ? e.message : String(e)));
  });
  const [menu, setMenu] = useState<MenuAt | null>(null);
  const closeMenu = useCallback(() => setMenu(null), []);
  // A right click at the pointer; Enter on a person opens it under them.
  const openMenu = (userId: string) => (e: MouseEvent<HTMLElement> | KeyboardEvent<HTMLElement>) => {
    e.preventDefault();
    const rect = e.currentTarget.getBoundingClientRect();
    const pointer = "clientX" in e && (e.clientX !== 0 || e.clientY !== 0);
    setMenu({ userId, x: pointer ? e.clientX : rect.left + 24, y: pointer ? e.clientY : rect.bottom });
  };
  const menuMember = menu === null ? undefined : status.members.find((m) => m.userId === menu.userId);
  const { phase } = status;

  return (
    <div className="app">
      <TitleBar />
      <main className="app__body">
        <section className={`status status--${phase.kind}`}>
          <Campfire size={44} server />
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
            {status.joinLink !== null && (
              <LinkRow label="Для друзей" hint="Отправьте её тем, кого зовёте на сервер." link={status.joinLink} />
            )}

            <Channels
              channels={status.channels}
              members={status.members}
              api={api}
              drag={drag}
              grab={grab}
              openMenu={openMenu}
            />
            {moveError && (
              <p className="member__error" role="alert">
                {moveError}
              </p>
            )}

            <section className="card">
              <h2 className="card__title">{crowd(status.members)}</h2>
              <ul className="list">
                {status.members.map((m) => (
                  <MemberRow key={m.userId} member={m} api={api} drag={drag} grab={grab} openMenu={openMenu} />
                ))}
              </ul>
            </section>
          </>
        )}

        <footer className="footer">
          <span>{voice(status)}</span>
          {status.address !== null && <span>Адрес: {status.address}</span>}
          {status.fingerprint !== null && <span className="footer__fp">Отпечаток: {status.fingerprint}</span>}
        </footer>
      </main>
      <DragGhost drag={drag} />
      {menu !== null && menuMember !== undefined && (
        <MemberMenu key={menu.userId} at={menu} member={menuMember} channels={status.channels} api={api} onClose={closeMenu} />
      )}
    </div>
  );
}
