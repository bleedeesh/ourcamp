import { useEffect, useState, type DragEvent } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Icon } from "@ourcamp/ui/Icon.tsx";
import { badgeText, unreadLabel } from "../../domain/chat.ts";
import { isTauri } from "../../engine/tauri-engine.ts";
import { Composer } from "./Composer.tsx";
import { Feed } from "./Feed.tsx";
import { useChat, useChatController } from "./chat-context.tsx";
import { makePhoto } from "./make-photo.ts";

/** The chat window (M5 step 3, ADR-0046): the mockup's screen 9. */
export function ChatApp() {
  const chat = useChatController();
  const status = useChat((s) => s.status);
  const [viewing, setViewing] = useState<string | null>(null);

  // A photo dropped anywhere on the window goes into the composer.
  const drop = (e: DragEvent) => {
    const file = [...e.dataTransfer.files].find((f) => f.type.startsWith("image/"));
    if (!file) return;
    e.preventDefault();
    makePhoto(file).then(
      (photo) => chat.attach(photo),
      (err: unknown) => chat.fail(err instanceof Error ? err.message : String(err)),
    );
  };

  return (
    <div className="chat-app" onDragOver={(e) => e.preventDefault()} onDrop={drop}>
      <ChatTitleBar />
      {status === "none" ? (
        <p className="chat-empty">Откройте сервер в главном окне — здесь появятся его чаты</p>
      ) : status === "loading" ? (
        <div className="chat-app__body" aria-busy="true" />
      ) : (
        <ChatBody onView={setViewing} />
      )}
      {viewing !== null && <PhotoViewer url={viewing} onClose={() => setViewing(null)} />}
    </div>
  );
}

function ChatTitleBar() {
  const campName = useChat((s) => s.campName);
  const minimize = () => {
    if (isTauri()) void getCurrentWindow().minimize();
  };
  // The chat window closes for real; the main window keeps the camp (ADR-0046).
  const close = () => {
    if (isTauri()) void getCurrentWindow().close();
    else window.close();
  };
  return (
    <header className="titlebar" data-tauri-drag-region>
      <span className="titlebar__brand titlebar__brand--chat" data-tauri-drag-region>
        <Icon name="chat" size={14} />
        <span className="titlebar__title" data-tauri-drag-region>
          {campName ? `Чат · ${campName}` : "Чат"}
        </span>
      </span>
      <button type="button" className="titlebar__button" aria-label="Свернуть" onClick={minimize}>
        <Icon name="minimize" size={14} />
      </button>
      <button type="button" className="titlebar__button titlebar__button--close" aria-label="Закрыть окно" onClick={close}>
        <Icon name="close" size={14} />
      </button>
    </header>
  );
}

function ChatBody({ onView }: { onView: (url: string) => void }) {
  const chats = useChat((s) => s.chats);
  return (
    <div className="chat-app__body">
      {chats.length === 0 ? (
        <p className="chat-empty">На этом сервере нет чатов — их добавляет владелец в окне сервера</p>
      ) : (
        <>
          <ChatTabs />
          <Feed onView={onView} />
          <Composer />
        </>
      )}
    </div>
  );
}

function ChatTabs() {
  const chat = useChatController();
  const chats = useChat((s) => s.chats);
  const activeId = useChat((s) => s.activeId);
  const latest = useChat((s) => s.latest);
  const read = useChat((s) => s.read);
  return (
    <div className="chat-tabs__wrap">
      <div className="chat-tabs" role="tablist" aria-label="Чаты сервера">
        {chats.map((c) => {
          const unread = c.id === activeId ? 0 : Math.max(0, (latest[c.id] ?? 0) - (read[c.id] ?? 0));
          return (
            <button
              key={c.id}
              type="button"
              role="tab"
              className="chat-tabs__tab"
              aria-selected={c.id === activeId}
              aria-label={unread > 0 ? `${c.name}, ${unreadLabel(unread)}` : c.name}
              onClick={() => void chat.select(c.id)}
            >
              <span className="chat-tabs__name">{c.name}</span>
              {unread > 0 && (
                <span className="unread-badge" aria-hidden="true">
                  {badgeText(unread)}
                </span>
              )}
            </button>
          );
        })}
      </div>
    </div>
  );
}

/** A photo over the whole window; a click or Esc closes it. */
function PhotoViewer({ url, onClose }: { url: string; onClose: () => void }) {
  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, [onClose]);
  return (
    <div className="photo-viewer" role="dialog" aria-modal="true" aria-label="Фото" onClick={onClose}>
      <img className="photo-viewer__image" src={url} alt="" />
      <button type="button" className="photo-viewer__close" aria-label="Закрыть фото" onClick={onClose}>
        <Icon name="close" size={18} />
      </button>
    </div>
  );
}
