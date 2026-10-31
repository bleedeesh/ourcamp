import { useRef, useState, type ChangeEvent } from "react";
import { avatarColor } from "../domain/roster.ts";
import { Icon } from "@ourcamp/ui/Icon.tsx";
import { makeAvatar } from "./avatar-picture.ts";
import { useBytesUrl } from "./avatar-url.ts";

interface AvatarPickerProps {
  /** Whose initial to show without a picture. */
  readonly name: string;
  readonly avatar: Uint8Array | null;
  readonly onChange: (picture: Uint8Array | null) => Promise<void>;
}

/**
 * Your avatar for the servers you enter (ADR-0035): kept on this device, sent to each
 * server. A click picks a picture; it is cut to a square and shrunk here.
 */
export function AvatarPicker({ name, avatar, onChange }: AvatarPickerProps) {
  const url = useBytesUrl(avatar);
  const fileRef = useRef<HTMLInputElement>(null);
  const [problem, setProblem] = useState<string | null>(null);

  const pick = async (e: ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    e.target.value = ""; // the same file again is a new pick
    if (!file) return;
    try {
      setProblem(null);
      await onChange(await makeAvatar(file));
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
          <button type="button" className="link-button" onClick={() => void onChange(null)}>
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
