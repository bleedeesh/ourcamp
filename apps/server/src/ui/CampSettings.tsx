import { useState, type FormEvent, type KeyboardEvent } from "react";
import { Campfire } from "@ourcamp/ui/Campfire.tsx";

import type { ServerApi } from "../api.ts";
import { MAX_CAMP_NAME, MAX_CHANNEL_NAME, nameProblem, type Channel } from "../status.ts";

const PENCIL = "M4 20h4L19 9l-4-4L4 16zM14 6l4 4";
const TRASH = "M4 7h16M10 11v6M14 11v6M5 7l1 13h12l1-13M9 7V4h6v3";
const HASH = "M5 9h14M5 15h14M10 3 8 21M16 3l-2 18";
const MOON = "M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z";

export function Glyph({ d, size = 14 }: { d: string; size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d={d} />
    </svg>
  );
}

/** Runs a change; a refusal shows next to the control instead of being lost. */
function useAction(): [error: string | null, run: (action: () => Promise<void>) => Promise<boolean>, clear: () => void] {
  const [error, setError] = useState<string | null>(null);
  const run = async (action: () => Promise<void>) => {
    try {
      await action();
      setError(null);
      return true;
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      return false;
    }
  };
  return [error, run, () => setError(null)];
}

/**
 * A name edited in place: Enter saves, Escape cancels. Checked here the way the camp
 * checks it, so a bad name never travels.
 */
function NameEditor({
  initial,
  max,
  label,
  onSave,
  onDone,
}: {
  initial: string;
  max: number;
  label: string;
  onSave: (name: string) => Promise<void>;
  onDone: () => void;
}) {
  const [value, setValue] = useState(initial);
  const [error, run] = useAction();
  const problem = nameProblem(value, max);
  const save = async (e?: FormEvent) => {
    e?.preventDefault();
    if (problem !== null) return;
    if (await run(() => onSave(value))) onDone();
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") onDone();
  };
  return (
    <form className="name-edit" onSubmit={(e) => void save(e)}>
      <input
        className="name-edit__input"
        aria-label={label}
        value={value}
        autoFocus
        maxLength={max * 2}
        onChange={(e) => setValue(e.target.value)}
        onKeyDown={onKey}
      />
      <button type="submit" className="small-button small-button--accent" disabled={problem !== null}>
        Сохранить
      </button>
      <button type="button" className="small-button" onClick={onDone}>
        Отмена
      </button>
      {(error ?? (value.trim() !== initial.trim() ? problem : null)) && (
        <span className="name-edit__error">{error ?? problem}</span>
      )}
    </form>
  );
}

/** The camp's name in the status card, with an edit button. */
export function CampName({ name, api }: { name: string; api: ServerApi }) {
  const [editing, setEditing] = useState(false);
  if (editing) {
    return (
      <NameEditor
        initial={name}
        max={MAX_CAMP_NAME}
        label="Название лагеря"
        onSave={(n) => api.renameCamp(n)}
        onDone={() => setEditing(false)}
      />
    );
  }
  return (
    <span className="camp-name">
      <h1 className="status__title">{name || "OurCamp Server"}</h1>
      <button type="button" className="icon-button" aria-label="Переименовать лагерь" onClick={() => setEditing(true)}>
        <Glyph d={PENCIL} />
      </button>
    </span>
  );
}

function ChannelIcon({ channel }: { channel: Channel }) {
  if (channel.afk) return <Glyph d={MOON} size={16} />;
  if (channel.kind === "text") return <Glyph d={HASH} size={16} />;
  return <Campfire size={16} simple />;
}

function ChannelRow({ channel, api }: { channel: Channel; api: ServerApi }) {
  const [mode, setMode] = useState<"view" | "rename" | "delete">("view");
  const [error, run, clear] = useAction();
  if (mode === "rename") {
    return (
      <li className="channel">
        <NameEditor
          initial={channel.name}
          max={MAX_CHANNEL_NAME}
          label={`Название канала ${channel.name}`}
          onSave={(n) => api.renameChannel(channel.id, n)}
          onDone={() => setMode("view")}
        />
      </li>
    );
  }
  if (mode === "delete") {
    return (
      <li className="channel channel--confirm" role="alertdialog" aria-label={`Удалить ${channel.name}`}>
        <span className="channel__question">
          Удалить «{channel.name}»?
          {channel.kind === "voice" && " Кто в нём сидит, выйдет из голоса."}
        </span>
        <span className="channel__actions">
          <button type="button" className="small-button small-button--danger" onClick={() => void run(() => api.deleteChannel(channel.id))}>
            Удалить
          </button>
          <button
            type="button"
            className="small-button"
            onClick={() => {
              clear();
              setMode("view");
            }}
          >
            Отмена
          </button>
        </span>
        {error && <span className="name-edit__error">{error}</span>}
      </li>
    );
  }
  return (
    <li className="channel">
      <span className="channel__icon">
        <ChannelIcon channel={channel} />
      </span>
      <span className="channel__name">{channel.name}</span>
      {channel.afk && <span className="channel__tag">системный</span>}
      <span className="channel__tools">
        <button type="button" className="icon-button" aria-label={`Переименовать ${channel.name}`} onClick={() => setMode("rename")}>
          <Glyph d={PENCIL} />
        </button>
        {!channel.afk && (
          <button type="button" className="icon-button icon-button--danger" aria-label={`Удалить ${channel.name}`} onClick={() => setMode("delete")}>
            <Glyph d={TRASH} />
          </button>
        )}
      </span>
    </li>
  );
}

function NewChannel({ api }: { api: ServerApi }) {
  const [name, setName] = useState("");
  const [kind, setKind] = useState<Channel["kind"]>("voice");
  const [error, run] = useAction();
  const problem = nameProblem(name, MAX_CHANNEL_NAME);
  const add = async (e: FormEvent) => {
    e.preventDefault();
    if (problem !== null) return;
    if (await run(() => api.createChannel(name, kind))) setName("");
  };
  return (
    <form className="new-channel" onSubmit={(e) => void add(e)}>
      <input
        className="name-edit__input"
        placeholder="Новый канал"
        aria-label="Название нового канала"
        value={name}
        maxLength={MAX_CHANNEL_NAME * 2}
        onChange={(e) => setName(e.target.value)}
      />
      <span className="segmented" role="radiogroup" aria-label="Тип канала">
        <button type="button" role="radio" aria-checked={kind === "voice"} onClick={() => setKind("voice")}>
          Костёр
        </button>
        <button type="button" role="radio" aria-checked={kind === "text"} onClick={() => setKind("text")}>
          Чат
        </button>
      </span>
      <button type="submit" className="small-button small-button--accent" disabled={problem !== null}>
        Добавить
      </button>
      {(error ?? (name.trim() !== "" ? problem : null)) && <span className="name-edit__error">{error ?? problem}</span>}
    </form>
  );
}

/** Campfires and chats of the camp, editable by its owner (ADR-0026). */
export function Channels({ channels, api }: { channels: readonly Channel[]; api: ServerApi }) {
  return (
    <section className="channels">
      <h2 className="members__title">Каналы</h2>
      <ul className="channels__list">
        {channels.map((c) => (
          <ChannelRow key={c.id} channel={c} api={api} />
        ))}
      </ul>
      <NewChannel api={api} />
      <p className="channels__hint">Чаты появятся у участников вместе с окном чата (M5).</p>
    </section>
  );
}
