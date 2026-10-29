import { useEffect, useRef, useState, type ChangeEvent, type FormEvent } from "react";
import { campfires, campShort } from "../domain/camps.ts";
import { avatarColor } from "../domain/roster.ts";
import { Icon } from "@ourcamp/ui/Icon.tsx";
import { makeAvatar } from "./avatar-picture.ts";
import { useBytesUrl } from "./avatar-url.ts";
import { useCamps, useCampsController } from "./camps-context.tsx";

/**
 * Your avatar for the servers you enter (ADR-0035): kept on this device, sent to each
 * server. A click picks a picture; it is cut to a square and shrunk here.
 */
function AvatarPicker({ name }: { name: string }) {
  const camps = useCampsController();
  const avatar = useCamps((s) => s.avatar);
  const url = useBytesUrl(avatar);
  const fileRef = useRef<HTMLInputElement>(null);
  const [problem, setProblem] = useState<string | null>(null);

  const pick = async (e: ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    e.target.value = ""; // the same file again is a new pick
    if (!file) return;
    try {
      setProblem(null);
      await camps.setAvatar(await makeAvatar(file));
    } catch (err) {
      setProblem(err instanceof Error ? err.message : String(err));
    }
  };

  return (
    <div className="avatar-picker">
      <button
        type="button"
        className="avatar-picker__button"
        aria-label={url === null ? "Выбрать аватарку" : "Сменить аватарку"}
        style={{ background: avatarColor(name.trim() || "?") }}
        onClick={() => fileRef.current?.click()}
      >
        {url === null ? (
          <span className="avatar-picker__initial">{name.trim().slice(0, 1).toUpperCase() || "?"}</span>
        ) : (
          <img className="avatar__picture" src={url} alt="" />
        )}
        <span className="avatar-picker__badge" aria-hidden="true">
          <Icon name="plus" size={14} />
        </span>
      </button>
      <span className="avatar-picker__text">
        <span className="avatar-picker__title">Аватарка</span>
        <span className="join__hint">{url === null ? "Необязательно — без неё будет буква" : "Её увидят все на сервере"}</span>
        {url !== null && (
          <button type="button" className="link-button" onClick={() => void camps.setAvatar(null)}>
            Убрать
          </button>
        )}
        {problem && (
          <span className="join__error" role="alert">
            {problem}
          </span>
        )}
      </span>
      <input ref={fileRef} type="file" accept="image/*" hidden onChange={(e) => void pick(e)} />
    </div>
  );
}

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
        {check.state === "checking" && <p className="join__hint">Проверяем сервер…</p>}
        {check.state === "error" && <p className="join__error">{check.message}</p>}
        {check.state === "ok" && (
          <div className="camp-card">
            <span className="camp-card__badge" style={{ background: avatarColor(check.preview.name) }}>
              {campShort(check.preview.name)}
            </span>
            <span className="camp-card__text">
              <span className="camp-card__name">{check.preview.name}</span>
              <span className="camp-card__sub">{campfires(check.preview.channels)}</span>
              {/* Admin keys are retired (ADR-0026): roles come from the server window. */}
              {check.preview.kind === "claim" && (
                <span className="join__error">
                  Ключ админа больше не действует — попросите ссылку для друзей, роль выдаст владелец сервера
                </span>
              )}
            </span>
          </div>
        )}
      </div>

      <AvatarPicker name={name} />

      <label className="field__label" htmlFor="join-name">
        Как вас называть
      </label>
      <div className="field">
        <input
          id="join-name"
          className="field__input"
          value={name}
          onChange={(e) => camps.setName(e.target.value)}
          placeholder="Имя на сервере"
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
        {joining ? "Входим…" : "Войти на сервер"}
      </button>
      <p className="join__hint">Ссылку присылает тот, у кого запущен сервер. Вставьте её целиком.</p>
    </form>
  );
}
