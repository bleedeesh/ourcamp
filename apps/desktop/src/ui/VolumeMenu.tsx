import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties } from "react";
import { canGovern, canMove } from "../domain/roster.ts";
import type { UserId } from "../domain/types.ts";
import { MAX_VOLUME, MIN_VOLUME, VOLUME_STEP } from "../domain/volume.ts";
import { useCampsController } from "./camps-context.tsx";
import { Avatar } from "./ChannelTree.tsx";
import { Icon } from "@ourcamp/ui/Icon.tsx";
import { useVoice, useVoiceController } from "./voice-context.tsx";

const MARGIN = 8;

/**
 * Moving, silencing and kicking from the member's menu (ADR-0029, 0033) — for moderators
 * and admins over the ranks they may act on; nothing at all for everyone else. Moving is
 * also the keyboard's way to do what dragging does.
 */
function Moderation({ userId, close }: { userId: UserId; close: () => void }) {
  const camps = useCampsController();
  const member = useVoice((s) => s.camp.members[userId]);
  const mayMove = useVoice((s) => userId !== s.selfId && canMove(s.camp, s.selfId, userId));
  const mayGovern = useVoice((s) => canGovern(s.camp, s.selfId, userId));
  const channels = useVoice((s) => s.camp.channels);
  const [open, setOpen] = useState<"none" | "move" | "kick">("none");
  if (!member || (!mayMove && !mayGovern)) return null;
  const here = channels.find((c) => c.memberIds.includes(userId))?.id;
  const targets = channels.filter((c) => c.id !== here);

  return (
    <div className="volume-menu__actions" role="group" aria-label="Модерация">
      {mayMove && here !== undefined && targets.length > 0 && (
        <>
          <button type="button" className="menu-button" aria-expanded={open === "move"} onClick={() => setOpen(open === "move" ? "none" : "move")}>
            <Icon name="move" size={16} />
            Перенести в канал
          </button>
          {open === "move" && (
            <div className="menu-sublist">
              {targets.map((c) => (
                <button
                  key={c.id}
                  type="button"
                  className="menu-button menu-button--sub"
                  onClick={() => {
                    close();
                    void camps.moveMember(userId, c.id);
                  }}
                >
                  {c.name}
                </button>
              ))}
            </div>
          )}
        </>
      )}
      {mayGovern && (
        <>
          <button
            type="button"
            className="menu-button"
            aria-pressed={member.silenced}
            onClick={() => {
              close();
              void camps.setSilenced(userId, !member.silenced);
            }}
          >
            <Icon name="mic" off={!member.silenced} size={16} />
            {member.silenced ? "Разрешить говорить" : "Запретить говорить"}
          </button>
          {open === "kick" ? (
            <div className="menu-confirm" role="alertdialog" aria-label={`Выгнать ${member.name}`}>
              <span>Выгнать «{member.name}» с сервера? Вернуться можно по приглашению.</span>
              <span className="menu-confirm__actions">
                <button
                  type="button"
                  className="menu-confirm__button menu-confirm__button--danger"
                  onClick={() => {
                    close();
                    void camps.kick(userId);
                  }}
                >
                  Выгнать
                </button>
                <button type="button" className="menu-confirm__button" onClick={() => setOpen("none")}>
                  Отмена
                </button>
              </span>
            </div>
          ) : (
            <button type="button" className="menu-button" data-danger onClick={() => setOpen("kick")}>
              <Icon name="kick" size={16} />
              Выгнать с сервера
            </button>
          )}
        </>
      )}
    </div>
  );
}

/** Per-member listening volume (0–200 %) and local mute, opened from a member row. */
export function VolumeMenu() {
  const voice = useVoiceController();
  const menu = useVoice((s) => s.volumeMenu);
  const member = useVoice((s) => (s.volumeMenu ? s.camp.members[s.volumeMenu.userId] : undefined));
  const prefs = useVoice((s) => (s.volumeMenu ? s.prefs[s.volumeMenu.userId] : undefined));
  const ref = useRef<HTMLDivElement>(null);
  const sliderRef = useRef<HTMLInputElement>(null);
  const [position, setPosition] = useState<{ left: number; top: number } | null>(null);

  // Keep the menu inside the (small) window — also when moderation opens a list in it.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!menu || !el) {
      setPosition(null);
      return;
    }
    const place = () => {
      const { width, height } = el.getBoundingClientRect();
      setPosition({
        left: Math.max(MARGIN, Math.min(menu.x, window.innerWidth - width - MARGIN)),
        top: Math.max(MARGIN, Math.min(menu.y, window.innerHeight - height - MARGIN)),
      });
    };
    place();
    const resized = new ResizeObserver(place);
    resized.observe(el);
    return () => resized.disconnect();
  }, [menu]);

  useEffect(() => {
    if (!menu) return;
    sliderRef.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") voice.closeVolumeMenu();
    };
    const onPointer = (e: PointerEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) voice.closeVolumeMenu();
    };
    document.addEventListener("keydown", onKey);
    document.addEventListener("pointerdown", onPointer);
    return () => {
      document.removeEventListener("keydown", onKey);
      document.removeEventListener("pointerdown", onPointer);
    };
  }, [menu, voice]);

  if (!menu || !member || !prefs) return null;

  return (
    <div
      ref={ref}
      className="volume-menu"
      role="dialog"
      aria-label={`Громкость: ${member.name}`}
      style={position ?? { left: menu.x, top: menu.y, visibility: "hidden" }}
    >
      <div className="volume-menu__head">
        <Avatar member={member} size={28} speaking={false} />
        <span className="volume-menu__name">{member.name}</span>
      </div>

      <label className="volume-menu__label" htmlFor="member-volume">
        Громкость для вас
        <output htmlFor="member-volume" className="volume-menu__value" data-muted={prefs.muted || undefined}>
          {prefs.muted ? "заглушен" : `${prefs.volume}%`}
        </output>
      </label>
      <input
        ref={sliderRef}
        id="member-volume"
        className="volume-slider"
        type="range"
        min={MIN_VOLUME}
        max={MAX_VOLUME}
        step={VOLUME_STEP}
        value={prefs.volume}
        disabled={prefs.muted}
        style={{ "--fill": `${(prefs.volume / MAX_VOLUME) * 100}%` } as CSSProperties}
        onChange={(e) => void voice.setVolume(menu.userId, Number(e.target.value))}
      />
      <div className="volume-menu__scale" aria-hidden>
        <span>0</span>
        <span>100</span>
        <span>200</span>
      </div>

      <div className="volume-menu__actions">
        <button
          type="button"
          className="menu-button"
          aria-pressed={prefs.muted}
          data-danger={prefs.muted || undefined}
          onClick={() => void voice.toggleLocalMute(menu.userId)}
        >
          <Icon name={prefs.muted ? "speaker" : "speakerOff"} size={16} />
          {prefs.muted ? "Включить звук" : "Заглушить для меня"}
        </button>
        <button type="button" className="menu-button" onClick={() => void voice.resetVolume(menu.userId)}>
          <Icon name="reset" size={16} />
          Сбросить
        </button>
      </div>

      <Moderation key={menu.userId} userId={menu.userId} close={() => voice.closeVolumeMenu()} />
    </div>
  );
}
