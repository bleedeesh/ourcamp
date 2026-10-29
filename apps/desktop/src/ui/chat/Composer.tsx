import { useEffect, useLayoutEffect, useRef, type ClipboardEvent, type KeyboardEvent } from "react";
import { Icon } from "@ourcamp/ui/Icon.tsx";
import { MAX_MESSAGE_CHARS } from "../../domain/chat-window.ts";
import { photoType, sizeLabel } from "../../domain/photo.ts";
import { useObjectUrl } from "../avatar-url.ts";
import { useChat, useChatController } from "./chat-context.tsx";
import { makePhoto } from "./make-photo.ts";

/** The counter shows from here on: room left is worth knowing near the end. */
const COUNT_FROM = MAX_MESSAGE_CHARS - 200;
/** The field grows with the text up to this, then scrolls. */
const MAX_FIELD_PX = 140;

/**
 * Writing in the active chat: Enter sends, Shift+Enter starts a new line; a photo comes
 * from the button, a paste or a drop. Forbidden to speak or offline, it says why.
 */
export function Composer() {
  const chat = useChatController();
  const draft = useChat((s) => s.draft);
  const photo = useChat((s) => s.photo);
  const sending = useChat((s) => s.sending);
  const error = useChat((s) => s.error);
  const silenced = useChat((s) => s.silenced);
  const offline = useChat((s) => s.offline);
  const activeName = useChat((s) => s.chats.find((c) => c.id === s.activeId)?.name ?? "");
  const activeId = useChat((s) => s.activeId);
  const canSend = useChat(() => chat.canSend());
  const field = useRef<HTMLTextAreaElement>(null);
  const picker = useRef<HTMLInputElement>(null);
  const length = [...draft].length;

  // The field follows the text's height, up to a limit.
  useLayoutEffect(() => {
    const el = field.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, MAX_FIELD_PX)}px`;
  }, [draft]);

  // Switching chats puts the cursor in the field.
  useEffect(() => {
    field.current?.focus();
  }, [activeId]);

  const attach = (file: File | null | undefined) => {
    if (!file) return;
    makePhoto(file).then(
      (made) => {
        chat.attach(made);
        field.current?.focus();
      },
      (err: unknown) => chat.fail(err instanceof Error ? err.message : String(err)),
    );
  };

  const key = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    // Not while an input method is still composing a word.
    if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
      e.preventDefault();
      void chat.send();
    }
  };

  const paste = (e: ClipboardEvent<HTMLTextAreaElement>) => {
    const file = [...e.clipboardData.files].find((f) => f.type.startsWith("image/"));
    if (!file) return;
    e.preventDefault();
    attach(file);
  };

  if (silenced || offline) {
    return (
      <div className="composer composer--locked" role="status">
        <Icon name={silenced ? "mic" : "link"} off={silenced} size={16} />
        {silenced ? "Вам запрещено говорить и писать на этом сервере" : "Сервер недоступен — сообщения не отправить"}
      </div>
    );
  }

  return (
    <div className="composer">
      {error && (
        <p className="composer__error" role="alert">
          {error}
        </p>
      )}
      {photo && <Attachment bytes={photo.bytes} onRemove={() => chat.detach()} />}
      <div className="composer__bar">
        <button type="button" className="composer__button" aria-label="Прикрепить фото" onClick={() => picker.current?.click()}>
          <Icon name="image" size={18} />
        </button>
        <input
          ref={picker}
          type="file"
          accept="image/*"
          hidden
          onChange={(e) => {
            attach(e.target.files?.[0]);
            e.target.value = "";
          }}
        />
        <label htmlFor="chat-message" className="sr-only">
          Сообщение
        </label>
        <textarea
          id="chat-message"
          ref={field}
          className="composer__field"
          rows={1}
          placeholder={activeName ? `Написать в «${activeName}»` : "Сообщение"}
          value={draft}
          onChange={(e) => chat.setDraft(e.target.value)}
          onKeyDown={key}
          onPaste={paste}
        />
        <button
          type="button"
          className="composer__button composer__button--send"
          aria-label="Отправить"
          disabled={!canSend}
          aria-busy={sending || undefined}
          onClick={() => void chat.send()}
        >
          <Icon name="send" size={16} />
        </button>
      </div>
      {length > COUNT_FROM && (
        <p className="composer__count" data-over={length > MAX_MESSAGE_CHARS || undefined}>
          {length} / {MAX_MESSAGE_CHARS}
        </p>
      )}
    </div>
  );
}

/** The photo about to go, with a way to take it back. */
function Attachment({ bytes, onRemove }: { bytes: Uint8Array; onRemove: () => void }) {
  // Freed when the photo is sent, removed or replaced.
  const url = useObjectUrl(bytes, photoType);
  return (
    <div className="attachment">
      {url !== null && <img className="attachment__thumb" src={url} alt="" />}
      <span className="attachment__label">Фото · {sizeLabel(bytes.length)}</span>
      <button type="button" className="attachment__remove" aria-label="Убрать фото" onClick={onRemove}>
        <Icon name="close" size={14} />
      </button>
    </div>
  );
}
