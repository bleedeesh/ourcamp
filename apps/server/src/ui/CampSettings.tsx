import { useState, type FormEvent, type KeyboardEvent, type MouseEvent, type PointerEvent } from "react";
import { Campfire } from "@ourcamp/ui/Campfire.tsx";
import { DROP_ATTRIBUTE, dropTarget } from "@ourcamp/ui/drag.ts";
import { Icon } from "@ourcamp/ui/Icon.tsx";
import type { Drag, Draggable } from "@ourcamp/ui/useDragMove.tsx";

import type { ServerApi } from "../api.ts";
import { MAX_CAMP_NAME, MAX_CHANNEL_NAME, nameProblem, type Channel, type Member } from "../status.ts";

/** Runs a change; a refusal shows next to the control instead of being lost. */
export function useAction(): [error: string | null, run: (action: () => Promise<void>) => Promise<boolean>, clear: () => void] {
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
        label="Название сервера"
        onSave={(n) => api.renameCamp(n)}
        onDone={() => setEditing(false)}
      />
    );
  }
  return (
    <span className="camp-name">
      <h1 className="status__title">{name || "OurCamp Server"}</h1>
      <button type="button" className="icon-button" aria-label="Переименовать сервер" onClick={() => setEditing(true)}>
        <Icon name="pencil" size={14} />
      </button>
    </span>
  );
}

function ChannelIcon({ channel }: { channel: Channel }) {
  if (channel.afk) return <Icon name="moon" size={16} />;
  if (channel.kind === "text") return <Icon name="hash" size={16} />;
  return <Campfire size={16} simple />;
}

/** Someone sitting in a campfire; drag them onto another one to move them (ADR-0029). */
function Seated({ member, channelId, drag, grab, openMenu }: { member: Member; channelId: string } & Dragging) {
  return (
    <li
      className="seated__person"
      tabIndex={0}
      aria-haspopup="dialog"
      data-dragging={drag?.userId === member.userId || undefined}
      onPointerDown={grab({ userId: member.userId, name: member.name, from: channelId })}
      onContextMenu={openMenu(member.userId)}
      onKeyDown={(e) => {
        if (e.key === "Enter") openMenu(member.userId)(e);
      }}
    >
      <span className="avatar" style={{ width: 22, height: 22, fontSize: 11 }} aria-hidden="true">
        {member.name.slice(0, 1).toUpperCase()}
      </span>
      <span className="seated__name">{member.name}</span>
      {member.silenced ? (
        <span className="member__silenced" title="Говорить запрещено">
          <Icon name="mic" off size={12} />
        </span>
      ) : (
        member.micMuted && (
          <span className="member__muted" title="Микрофон выключен">
            <Icon name="mic" off size={14} />
          </span>
        )
      )}
    </li>
  );
}

function ChannelRow({
  channel,
  seated,
  api,
  drag,
  grab,
  openMenu,
}: { channel: Channel; seated: readonly Member[]; api: ServerApi } & Dragging) {
  const [mode, setMode] = useState<"view" | "rename" | "delete">("view");
  const [error, run, clear] = useAction();
  const voice = channel.kind === "voice";
  const dropHere = voice && drag !== null && dropTarget(drag.from, drag.over) === channel.id;

  let row;
  if (mode === "rename") {
    row = (
      <NameEditor
        initial={channel.name}
        max={MAX_CHANNEL_NAME}
        label={`Название канала ${channel.name}`}
        onSave={(n) => api.renameChannel(channel.id, n)}
        onDone={() => setMode("view")}
      />
    );
  } else if (mode === "delete") {
    row = (
      <>
        <span className="channel__question">
          Удалить «{channel.name}»?
          {voice && seated.length > 0 && " Кто в нём сидит, выйдет из голоса."}
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
      </>
    );
  } else {
    row = (
      <>
        <span className="channel__icon">
          <ChannelIcon channel={channel} />
        </span>
        <span className="channel__name">{channel.name}</span>
        {channel.afk && <span className="channel__tag">системный</span>}
        {seated.length > 0 && <span className="channel__count">{seated.length}</span>}
        <span className="channel__tools">
          <button type="button" className="icon-button" aria-label={`Переименовать ${channel.name}`} onClick={() => setMode("rename")}>
            <Icon name="pencil" size={14} />
          </button>
          {!channel.afk && (
            <button type="button" className="icon-button icon-button--danger" aria-label={`Удалить ${channel.name}`} onClick={() => setMode("delete")}>
              <Icon name="trash" size={14} />
            </button>
          )}
        </span>
      </>
    );
  }

  return (
    <li
      className={mode === "delete" ? "channel channel--confirm" : "channel"}
      role={mode === "delete" ? "alertdialog" : undefined}
      aria-label={mode === "delete" ? `Удалить ${channel.name}` : undefined}
      {...(voice ? { [DROP_ATTRIBUTE]: channel.id } : {})}
      data-drop-target={dropHere || undefined}
    >
      <div className="channel__row">{row}</div>
      {seated.length > 0 && (
        <ul className="seated" aria-label={`В канале ${channel.name}`}>
          {seated.map((m) => (
            <Seated key={m.userId} member={m} channelId={channel.id} drag={drag} grab={grab} openMenu={openMenu} />
          ))}
        </ul>
      )}
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
          Голосовой
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

/** What a list of people needs to let them be dragged onto channels (ADR-0029). */
export interface Dragging {
  readonly drag: Drag | null;
  readonly grab: (who: Draggable) => (e: PointerEvent<HTMLElement>) => void;
  /** Opens the person's menu (ADR-0034): a right click, or Enter on the person. */
  readonly openMenu: (userId: string) => (e: MouseEvent<HTMLElement> | KeyboardEvent<HTMLElement>) => void;
}

/**
 * Campfires and chats of the camp, editable by its owner (ADR-0026), with who sits in
 * each; dragging someone onto another campfire moves them there (ADR-0029).
 */
export function Channels({
  channels,
  members,
  api,
  drag,
  grab,
  openMenu,
}: { channels: readonly Channel[]; members: readonly Member[]; api: ServerApi } & Dragging) {
  const seatedIn = (id: string) => members.filter((m) => m.campfireId === id);
  const anyoneSeated = members.some((m) => m.campfireId !== null);
  return (
    <section className="card">
      <h2 className="card__title">Каналы</h2>
      <ul className="list">
        {channels.map((c) => (
          <ChannelRow key={c.id} channel={c} seated={seatedIn(c.id)} api={api} drag={drag} grab={grab} openMenu={openMenu} />
        ))}
      </ul>
      {anyoneSeated && <p className="card__hint">Перетащите участника в другой канал — он перейдёт туда.</p>}
      <NewChannel api={api} />
      <p className="card__hint">Чаты появятся у участников вместе с окном чата (M5).</p>
    </section>
  );
}
