import { useEffect, useRef } from "react";
import { campShort } from "../domain/camps.ts";
import { avatarColor } from "../domain/roster.ts";
import { Icon } from "./icons.tsx";
import { useCamps, useCampsController } from "./camps-context.tsx";
import { useVoice } from "./voice-context.tsx";

export function CampHeader() {
  const campsCtl = useCampsController();
  const camp = useVoice((s) => s.camp);
  const open = useCamps((s) => s.switcherOpen);
  return (
    <div className="camp-header">
      <button
        type="button"
        className="camp-switcher"
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => campsCtl.toggleSwitcher()}
      >
        <span className="camp-switcher__badge" style={{ background: camp.color }}>
          {camp.short}
        </span>
        <span className="camp-switcher__name">{camp.name}</span>
        <Icon name="chevron" size={16} className="camp-switcher__chevron" />
      </button>
      {/* Friends arrive with accounts (roadmap M4). */}
      <button type="button" className="icon-button" aria-label="Друзья" aria-disabled="true" title="Скоро">
        <Icon name="users" />
      </button>
      {open && <CampMenu />}
    </div>
  );
}

/** Mockup screen 8: joined camps, invite, join another. */
function CampMenu() {
  const campsCtl = useCampsController();
  const camps = useCamps((s) => s.camps);
  const currentId = useCamps((s) => s.currentId);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    ref.current?.querySelector<HTMLButtonElement>("button")?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") campsCtl.closeSwitcher();
    };
    const onPointer = (e: PointerEvent) => {
      const target = e.target as Element | null;
      // The switcher button toggles the menu itself.
      if (ref.current && !ref.current.contains(target) && !target?.closest(".camp-switcher")) campsCtl.closeSwitcher();
    };
    document.addEventListener("keydown", onKey);
    document.addEventListener("pointerdown", onPointer);
    return () => {
      document.removeEventListener("keydown", onKey);
      document.removeEventListener("pointerdown", onPointer);
    };
  }, [campsCtl]);

  return (
    <div ref={ref} className="camp-menu" role="menu" aria-label="Лагеря">
      {camps.map((c) => (
        <button
          key={c.id}
          type="button"
          role="menuitemradio"
          aria-checked={c.id === currentId}
          className="camp-menu__camp"
          onClick={() => void campsCtl.switchTo(c.id)}
        >
          <span className="camp-switcher__badge" style={{ background: avatarColor(c.id) }}>
            {campShort(c.name)}
          </span>
          <span className="camp-menu__text">
            <span className="camp-menu__name">{c.name}</span>
            <span className="camp-menu__sub">{c.role === "admin" ? "Вы админ" : "Участник"}</span>
          </span>
          {c.id === currentId && <Icon name="check" size={18} className="camp-menu__check" />}
        </button>
      ))}
      <span className="camp-menu__sep" role="separator" />
      <button type="button" role="menuitem" className="menu-button" onClick={() => void campsCtl.copyInvite()}>
        <Icon name="copy" size={18} />
        Скопировать приглашение
      </button>
      <button type="button" role="menuitem" className="menu-button" onClick={() => campsCtl.openJoin()}>
        <Icon name="link" size={18} />
        Войти по ссылке
      </button>
      <button type="button" role="menuitem" className="menu-button" aria-disabled="true" title="Скоро: окно сервера лагеря">
        <Icon name="plus" size={18} />
        Создать свой лагерь
      </button>
    </div>
  );
}
