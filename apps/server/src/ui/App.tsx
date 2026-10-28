import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Campfire } from "@ourcamp/ui/Campfire.tsx";
import { copyText } from "@ourcamp/ui/clipboard.ts";

import { isTauri, type ServerApi } from "../api.ts";
import { crowd, headline, STARTING, voice, type Member, type ServerStatus } from "../status.ts";

function Glyph({ d }: { d: string }) {
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d={d} />
    </svg>
  );
}

const MINIMIZE = "M5 12h14";
const CLOSE = "M18 6 6 18M6 6l12 12";
const CROWN = "M3 18h18M4 18 3 8l5 4 4-7 4 7 5-4-1 10";
const SHIELD = "M12 3 4 6v6c0 4.5 3.4 8.3 8 9 4.6-.7 8-4.5 8-9V6z";
const MIC_OFF = "M2 2l20 20M9 9v3a3 3 0 0 0 5.1 2.1M15 9.3V5a3 3 0 0 0-5.9-.6M19 10v2a7 7 0 0 1-.6 2.9M5 10v2a7 7 0 0 0 11.5 5.4M12 19v3";

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
        <Glyph d={MINIMIZE} />
      </button>
      <button
        type="button"
        className="titlebar__button titlebar__button--close"
        aria-label="Закрыть"
        onClick={() => void win()?.close()}
      >
        <Glyph d={CLOSE} />
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

function MemberRow({ member }: { member: Member }) {
  return (
    <li className="member">
      <span className="member__avatar" aria-hidden="true">
        {member.name.slice(0, 1).toUpperCase()}
      </span>
      <span className="member__name">{member.name}</span>
      {member.role === "admin" && (
        <span className="member__crown" title="Админ">
          <Glyph d={CROWN} />
        </span>
      )}
      {member.role === "moderator" && (
        <span className="member__shield" title="Модератор">
          <Glyph d={SHIELD} />
        </span>
      )}
      {member.campfire !== null && member.micMuted && (
        <span className="member__muted" title="Микрофон выключен">
          <Glyph d={MIC_OFF} />
        </span>
      )}
      <span className={member.campfire === null ? "member__where" : "member__where member__where--seated"}>
        {member.campfire ?? "не в голосе"}
      </span>
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
            <h1 className="status__title">{status.campName || "OurCamp Server"}</h1>
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
                <LinkRow label="Для друзей" hint="Отправьте её тем, кого зовёте в лагерь." link={status.joinLink} />
              )}
            </section>

            <section className="members">
              <h2 className="members__title">{crowd(status.members)}</h2>
              <ul className="members__list">
                {status.members.map((m) => (
                  <MemberRow key={m.userId} member={m} />
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
    </div>
  );
}
