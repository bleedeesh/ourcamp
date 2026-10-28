import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties } from "react";
import { MAX_VOLUME, MIN_VOLUME, VOLUME_STEP } from "../domain/volume.ts";
import { Avatar } from "./ChannelTree.tsx";
import { Icon } from "@ourcamp/ui/Icon.tsx";
import { useVoice, useVoiceController } from "./voice-context.tsx";

const MARGIN = 8;

/** Per-member listening volume (0–200 %) and local mute, opened from a member row. */
export function VolumeMenu() {
  const voice = useVoiceController();
  const menu = useVoice((s) => s.volumeMenu);
  const member = useVoice((s) => (s.volumeMenu ? s.camp.members[s.volumeMenu.userId] : undefined));
  const prefs = useVoice((s) => (s.volumeMenu ? s.prefs[s.volumeMenu.userId] : undefined));
  const ref = useRef<HTMLDivElement>(null);
  const sliderRef = useRef<HTMLInputElement>(null);
  const [position, setPosition] = useState<{ left: number; top: number } | null>(null);

  // Keep the menu inside the (small) window.
  useLayoutEffect(() => {
    if (!menu || !ref.current) {
      setPosition(null);
      return;
    }
    const { width, height } = ref.current.getBoundingClientRect();
    setPosition({
      left: Math.max(MARGIN, Math.min(menu.x, window.innerWidth - width - MARGIN)),
      top: Math.max(MARGIN, Math.min(menu.y, window.innerHeight - height - MARGIN)),
    });
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
    </div>
  );
}
