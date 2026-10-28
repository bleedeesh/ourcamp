import { getCurrentWindow } from "@tauri-apps/api/window";
import { isTauri } from "../engine/tauri-engine.ts";
import { Campfire } from "@ourcamp/ui/Campfire.tsx";
import { Icon } from "@ourcamp/ui/Icon.tsx";

/**
 * Custom window chrome (the native one is disabled in tauri.conf.json so the title bar
 * matches the app). `data-tauri-drag-region` makes the bar move the window.
 */
export function TitleBar() {
  const minimize = () => {
    if (isTauri()) void getCurrentWindow().minimize();
  };
  // Rust turns closing into hiding: OurCamp stays in the tray (ADR-0031).
  const close = () => {
    if (isTauri()) void getCurrentWindow().close();
  };
  return (
    <header className="titlebar" data-tauri-drag-region>
      <span className="titlebar__brand" data-tauri-drag-region>
        <Campfire size={14} simple />
        <span className="titlebar__name" data-tauri-drag-region>
          OurCamp
        </span>
      </span>
      <button type="button" className="titlebar__button" aria-label="Свернуть" onClick={minimize}>
        <Icon name="minimize" size={14} />
      </button>
      <button type="button" className="titlebar__button titlebar__button--close" aria-label="Свернуть в трей" title="Свернуть в трей" onClick={close}>
        <Icon name="close" size={14} />
      </button>
    </header>
  );
}
