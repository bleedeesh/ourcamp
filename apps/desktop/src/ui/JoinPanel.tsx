import { useEffect, useRef, type FormEvent } from "react";
import { campfires, campShort } from "../domain/camps.ts";
import { avatarColor } from "../domain/roster.ts";
import { Icon } from "./icons.tsx";
import { useCamps, useCampsController } from "./camps-context.tsx";

/** "Войти по ссылке": paste an invite, see the camp it leads to, choose a name. */
export function JoinPanel() {
  const camps = useCampsController();
  const link = useCamps((s) => s.link);
  const name = useCamps((s) => s.name);
  const check = useCamps((s) => s.check);
  const joining = useCamps((s) => s.joining);
  const joinError = useCamps((s) => s.joinError);
  const canJoin = useCamps(() => camps.canJoin());
  const linkRef = useRef<HTMLInputElement>(null);

  useEffect(() => linkRef.current?.focus(), []);

  const paste = async () => {
    try {
      const text = await navigator.clipboard.readText();
      void camps.setLink(text);
    } catch {
      // Clipboard access denied: fall back to Ctrl+V into the field.
      linkRef.current?.focus();
    }
  };

  const submit = (e: FormEvent) => {
    e.preventDefault();
    void camps.join();
  };

  return (
    <form className="join" onSubmit={submit} aria-labelledby="join-title">
      <div className="join__head">
        <button type="button" className="icon-button" aria-label="Назад" onClick={() => camps.cancelJoin()}>
          <Icon name="back" />
        </button>
        <h2 id="join-title" className="join__title">
          Войти по ссылке
        </h2>
      </div>

      <label className="field__label" htmlFor="join-link">
        Ссылка или код приглашения
      </label>
      <div className="field">
        <input
          id="join-link"
          ref={linkRef}
          className="field__input"
          value={link}
          onChange={(e) => void camps.setLink(e.target.value)}
          placeholder="ourcamp://…"
          spellCheck={false}
          autoComplete="off"
          aria-describedby="join-status"
          aria-invalid={check.state === "error" || undefined}
        />
        <button type="button" className="field__action" aria-label="Вставить ссылку из буфера" onClick={() => void paste()}>
          <Icon name="paste" size={18} />
        </button>
      </div>

      <div id="join-status" className="join__status" aria-live="polite">
        {check.state === "checking" && <p className="join__hint">Проверяем лагерь…</p>}
        {check.state === "error" && <p className="join__error">{check.message}</p>}
        {check.state === "ok" && (
          <div className="camp-card">
            <span className="camp-card__badge" style={{ background: avatarColor(check.preview.name) }}>
              {campShort(check.preview.name)}
            </span>
            <span className="camp-card__text">
              <span className="camp-card__name">{check.preview.name}</span>
              <span className="camp-card__sub">{campfires(check.preview.channels)}</span>
              {check.preview.kind === "claim" &&
                (check.preview.usable ? (
                  <span className="camp-card__claim">
                    <Icon name="crown" size={14} /> Ключ админа: вы станете админом лагеря
                  </span>
                ) : (
                  <span className="join__error">Этот ключ админа уже использован</span>
                ))}
            </span>
          </div>
        )}
      </div>

      <label className="field__label" htmlFor="join-name">
        Как вас называть
      </label>
      <div className="field">
        <input
          id="join-name"
          className="field__input"
          value={name}
          onChange={(e) => camps.setName(e.target.value)}
          placeholder="Имя в лагере"
          maxLength={32}
          autoComplete="nickname"
        />
      </div>

      {joinError && (
        <p className="join__error" role="alert">
          {joinError}
        </p>
      )}

      <button type="submit" className="primary-button" aria-disabled={!canJoin || undefined} disabled={!canJoin}>
        {joining ? "Входим…" : "Войти в лагерь"}
      </button>
      <p className="join__hint">Ссылку присылает тот, у кого запущен сервер лагеря. Вставьте её целиком.</p>
    </form>
  );
}
