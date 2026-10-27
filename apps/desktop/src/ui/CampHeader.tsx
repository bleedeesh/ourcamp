import { Icon } from "./icons.tsx";
import { useVoice } from "./voice-context.tsx";

export function CampHeader() {
  const camp = useVoice((s) => s.camp);
  return (
    <div className="camp-header">
      {/* The camp switcher menu arrives with multi-camp support (roadmap M4). */}
      <button type="button" className="camp-switcher" aria-haspopup="menu" aria-disabled="true">
        <span className="camp-switcher__badge" style={{ background: camp.color }}>
          {camp.short}
        </span>
        <span className="camp-switcher__name">{camp.name}</span>
        <Icon name="chevron" size={16} className="camp-switcher__chevron" />
      </button>
      <button type="button" className="icon-button" aria-label="Друзья" aria-disabled="true" title="Скоро">
        <Icon name="users" />
      </button>
    </div>
  );
}
