import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Icon } from "@ourcamp/ui/Icon.tsx";
import { layoutFeed, timeLabel, type FeedItem } from "../../domain/chat-feed.ts";
import type { ChatMember } from "../../domain/chat-window.ts";
import { photoType } from "../../domain/photo.ts";
import { avatarColor } from "../../domain/roster.ts";
import type { ChatMessage, ChatPhoto } from "../../engine/camp-api.ts";
import { Avatar } from "../Avatar.tsx";
import { useChat, useChatController } from "./chat-context.tsx";

/** Closer to the bottom than this counts as "at the end": new messages keep it there. */
const AT_END_PX = 48;
/** Closer to the top than this fetches the page before. */
const NEAR_TOP_PX = 240;

/**
 * The active chat's messages, newest at the bottom. It stays at the end as messages
 * arrive unless you scrolled up to read; scrolling to the top fetches older pages
 * without the view jumping; seeing the end with the window focused marks it read.
 */
export function Feed({ onView }: { onView: (url: string) => void }) {
  const chat = useChatController();
  const activeId = useChat((s) => s.activeId);
  const thread = useChat((s) => (s.activeId === null ? undefined : s.threads[s.activeId]));
  const selfId = useChat((s) => s.selfId);
  const members = useChat((s) => s.members);
  const [now, setNow] = useState(() => Date.now());
  const scroller = useRef<HTMLDivElement>(null);
  const atEnd = useRef(true);
  /** Height and first message before an update: older pages keep the view in place. */
  const before = useRef<{ height: number; top: number; first: number | undefined } | null>(null);
  const [showJump, setShowJump] = useState(false);

  const messages = thread?.messages;
  const items = useMemo(
    () => (messages ? layoutFeed(messages, now, selfId, thread?.unreadAfter ?? null) : []),
    [messages, now, selfId, thread?.unreadAfter],
  );

  // "Сегодня" turns into "Вчера" at midnight even with the window open.
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 60_000);
    return () => clearInterval(timer);
  }, []);

  const seenIfAtEnd = useCallback(() => {
    if (atEnd.current && document.visibilityState === "visible" && document.hasFocus()) chat.markSeen();
  }, [chat]);

  // A chat opened (or switched to) starts at its end, with nothing remembered of the last.
  useLayoutEffect(() => {
    atEnd.current = true;
    before.current = null;
    setShowJump(false);
  }, [activeId]);

  // After an update: keep the place when older messages came in above, follow the end
  // when you were there.
  useLayoutEffect(() => {
    const el = scroller.current;
    if (!el || !messages) return;
    const was = before.current;
    const first = messages[0]?.seq;
    const olderCameIn = was?.first !== undefined && first !== undefined && first < was.first;
    if (olderCameIn && was) {
      el.scrollTop = el.scrollHeight - was.height + was.top;
    } else if (atEnd.current) {
      el.scrollTop = el.scrollHeight;
    }
    before.current = { height: el.scrollHeight, top: el.scrollTop, first: messages[0]?.seq };
    seenIfAtEnd();
  }, [messages, seenIfAtEnd]);

  // The composer grows (a photo, more lines) or the window is resized: at the end, stay.
  useEffect(() => {
    const el = scroller.current;
    if (!el) return;
    const keep = new ResizeObserver(() => {
      if (atEnd.current) el.scrollTop = el.scrollHeight;
    });
    keep.observe(el);
    return () => keep.disconnect();
  }, []);

  useEffect(() => {
    window.addEventListener("focus", seenIfAtEnd);
    document.addEventListener("visibilitychange", seenIfAtEnd);
    return () => {
      window.removeEventListener("focus", seenIfAtEnd);
      document.removeEventListener("visibilitychange", seenIfAtEnd);
    };
  }, [seenIfAtEnd]);

  const scrolled = () => {
    const el = scroller.current;
    if (!el) return;
    atEnd.current = el.scrollHeight - el.scrollTop - el.clientHeight < AT_END_PX;
    before.current = { height: el.scrollHeight, top: el.scrollTop, first: messages?.[0]?.seq };
    setShowJump(!atEnd.current);
    if (el.scrollTop < NEAR_TOP_PX) void chat.loadOlder();
    seenIfAtEnd();
  };

  const jumpToEnd = () => {
    const el = scroller.current;
    if (el) el.scrollTo({ top: el.scrollHeight, behavior: "smooth" });
  };

  return (
    <div className="feed">
      <div className="feed__scroller" ref={scroller} onScroll={scrolled} role="log" aria-live="polite" aria-label="Сообщения">
        <div className="feed__spacer" />
        {thread?.loading && (messages?.length ?? 0) > 0 && <p className="feed__note">Загружаем…</p>}
        {thread?.complete && (messages?.length ?? 0) > 0 && <p className="feed__note">Начало чата</p>}
        {thread?.error && <p className="feed__note feed__note--error">{thread.error}</p>}
        {thread && !thread.loading && !thread.error && messages?.length === 0 && (
          <p className="feed__quiet">Здесь пока тихо — напишите первым</p>
        )}
        {items.map((item) => (
          <FeedRow key={item.key} item={item} members={members} onView={onView} />
        ))}
      </div>
      {showJump && (
        <button type="button" className="feed__jump" aria-label="К новым сообщениям" onClick={jumpToEnd}>
          <Icon name="chevron" size={18} />
        </button>
      )}
    </div>
  );
}

function FeedRow({
  item,
  members,
  onView,
}: {
  item: FeedItem;
  members: Readonly<Record<string, ChatMember>>;
  onView: (url: string) => void;
}) {
  if (item.kind === "day") return <p className="feed__day">{item.label}</p>;
  if (item.kind === "unread") {
    return (
      <p className="feed__unread">
        <span>Новые сообщения</span>
      </p>
    );
  }
  return <MessageRow message={item.message} head={item.head} member={members[item.message.authorId]} onView={onView} />;
}

function MessageRow({
  message,
  head,
  member,
  onView,
}: {
  message: ChatMessage;
  head: boolean;
  member: ChatMember | undefined;
  onView: (url: string) => void;
}) {
  // Someone who left keeps the name they wrote with.
  const person = {
    id: message.authorId,
    name: member?.name ?? message.authorName,
    color: member?.color ?? avatarColor(message.authorId),
    avatar: member?.avatar ?? null,
  };
  const time = timeLabel(message.sentAt);
  return (
    <article className="message" data-head={head || undefined}>
      {head ? (
        <Avatar member={person} size={30} speaking={false} />
      ) : (
        <time className="message__gutter-time" dateTime={new Date(message.sentAt).toISOString()}>
          {time}
        </time>
      )}
      <div className="message__body">
        {head && (
          <div className="message__head">
            <span className="message__author">{person.name}</span>
            <time className="message__time" dateTime={new Date(message.sentAt).toISOString()}>
              {time}
            </time>
          </div>
        )}
        {message.text !== "" && <p className="message__text">{message.text}</p>}
        {message.photo && <MessagePhoto photo={message.photo} onView={onView} />}
      </div>
    </article>
  );
}

/** Object URLs of photos by hash, for the life of the window: each is decoded once. */
const photoUrls = new Map<string, string>();

function MessagePhoto({ photo, onView }: { photo: ChatPhoto; onView: (url: string) => void }) {
  const chat = useChatController();
  const [url, setUrl] = useState<string | null>(() => photoUrls.get(photo.hash) ?? null);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    if (photoUrls.has(photo.hash)) return;
    let live = true;
    chat.photoOf(photo.hash).then(
      (bytes) => {
        const type = photoType(bytes);
        if (type === null) throw new Error("not a photo");
        const made = photoUrls.get(photo.hash) ?? URL.createObjectURL(new Blob([bytes as BlobPart], { type }));
        photoUrls.set(photo.hash, made);
        if (live) setUrl(made);
      },
      () => {
        if (live) setFailed(true);
      },
    );
    return () => {
      live = false;
    };
  }, [chat, photo.hash]);
  // The box has the photo's shape before it loads: the feed does not jump.
  const style = { aspectRatio: `${photo.width} / ${photo.height}`, width: Math.min(photo.width, 320) };
  return (
    <button
      type="button"
      className="message__photo"
      style={style}
      aria-label="Открыть фото"
      disabled={url === null}
      onClick={() => url !== null && onView(url)}
    >
      {url !== null ? (
        <img src={url} alt="" draggable={false} />
      ) : (
        <span className="message__photo-wait">{failed ? "Фото не загрузилось" : ""}</span>
      )}
    </button>
  );
}
