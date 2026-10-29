import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { Icon } from "@ourcamp/ui/Icon.tsx";

import type { ServerApi } from "../api.ts";
import type { Channel, Member } from "../status.ts";
import { useAction } from "./CampSettings.tsx";
import { MemberAvatar } from "./MemberAvatar.tsx";

/** Where the menu opened, and for whom. */
export interface MenuAt {
  readonly userId: string;
  readonly x: number;
  readonly y: number;
}

const MARGIN = 8;

/**
 * The owner's right-click menu on a person (ADR-0034): move to a channel, forbid or allow
 * speaking, kick — the same as the row's buttons, where the person is (in the channel
 * tree too) and with the keyboard's way to move.
 */
export function MemberMenu({
  at,
  member,
  channels,
  api,
  onClose,
}: {
  at: MenuAt;
  member: Member;
  channels: readonly Channel[];
  api: ServerApi;
  onClose: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState<{ left: number; top: number } | null>(null);
  const [open, setOpen] = useState<"none" | "move" | "kick">("none");
  const [error, run] = useAction();

  // Inside the window, also when a list opens in it.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const place = () => {
      const { width, height } = el.getBoundingClientRect();
      setPosition({
        left: Math.max(MARGIN, Math.min(at.x, window.innerWidth - width - MARGIN)),
        top: Math.max(MARGIN, Math.min(at.y, window.innerHeight - height - MARGIN)),
      });
    };
    place();
    const resized = new ResizeObserver(place);
    resized.observe(el);
    return () => resized.disconnect();
  }, [at]);

  useEffect(() => {
    ref.current?.querySelector("button")?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    const onPointer = (e: PointerEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose();
    };
    document.addEventListener("keydown", onKey);
    document.addEventListener("pointerdown", onPointer);
    return () => {
      document.removeEventListener("keydown", onKey);
      document.removeEventListener("pointerdown", onPointer);
    };
  }, [onClose]);

  const act = async (action: () => Promise<void>) => {
    if (await run(action)) onClose();
  };
  const targets = channels.filter((c) => c.kind === "voice" && c.id !== member.campfireId);

  return (
    <div
      ref={ref}
      className="context-menu"
      role="dialog"
      aria-label={`Действия: ${member.name}`}
      style={position ?? { left: at.x, top: at.y, visibility: "hidden" }}
    >
      <div className="context-menu__head">
        <MemberAvatar member={member} size={24} api={api} />
        <span className="context-menu__name">{member.name}</span>
      </div>
      {member.campfireId !== null && targets.length > 0 && (
        <>
          <button type="button" className="context-menu__item" aria-expanded={open === "move"} onClick={() => setOpen(open === "move" ? "none" : "move")}>
            <Icon name="move" size={16} />
            Перенести в канал
          </button>
          {open === "move" &&
            targets.map((c) => (
              <button
                key={c.id}
                type="button"
                className="context-menu__item context-menu__item--sub"
                onClick={() => void act(() => api.moveMember(member.userId, c.id))}
              >
                {c.name}
              </button>
            ))}
        </>
      )}
      <button
        type="button"
        className="context-menu__item"
        aria-pressed={member.silenced}
        onClick={() => void act(() => api.setSilenced(member.userId, !member.silenced))}
      >
        <Icon name="mic" off={!member.silenced} size={16} />
        {member.silenced ? "Разрешить говорить" : "Запретить говорить"}
      </button>
      {open === "kick" ? (
        <div className="context-menu__confirm" role="alertdialog" aria-label={`Выгнать ${member.name}`}>
          <span>Выгнать «{member.name}» с сервера? Вернуться можно по ссылке для друзей.</span>
          <span className="member__actions">
            <button type="button" className="small-button small-button--danger" onClick={() => void act(() => api.kick(member.userId))}>
              Выгнать
            </button>
            <button type="button" className="small-button" onClick={() => setOpen("none")}>
              Отмена
            </button>
          </span>
        </div>
      ) : (
        <button type="button" className="context-menu__item" data-danger onClick={() => setOpen("kick")}>
          <Icon name="kick" size={16} />
          Выгнать с сервера
        </button>
      )}
      {error && <span className="name-edit__error">{error}</span>}
    </div>
  );
}
