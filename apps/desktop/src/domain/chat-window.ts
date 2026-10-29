import type {
  CampApi,
  CampLive,
  CampState,
  ChannelInfo,
  ChatContext,
  ChatMessage,
  ChatUpdate,
  OutgoingPhoto,
} from "../engine/camp-api.ts";
import { createStore, type Store } from "../lib/store.ts";
import { advance, type SeqByChat } from "./chat.ts";
import { gapBefore, mergeMessages } from "./chat-feed.ts";
import { avatarColor } from "./roster.ts";
import type { UserId } from "./types.ts";
import type { KeyValueStorage } from "./volume.ts";

/** Messages per history request: a screenful and then some. */
export const PAGE = 50;
/** The longest message, as the camp counts it (`oc_proto::MAX_MESSAGE_CHARS`). */
export const MAX_MESSAGE_CHARS = 2000;
/** Where the window remembers the chat it showed last, per camp. */
const LAST_CHAT_KEY = "ourcamp.chat.last.v1";

/** One chat as the window holds it. */
export interface ChatThread {
  /** Oldest first, without gaps from the first one held to the last. */
  readonly messages: readonly ChatMessage[];
  readonly loading: boolean;
  /** The chat's first message is held: nothing older to fetch. */
  readonly complete: boolean;
  readonly error: string | null;
  /** The last message read when the chat was opened here: the "new" line goes after it. */
  readonly unreadAfter: number | null;
}

/** A camp member as the feed shows them. */
export interface ChatMember {
  readonly name: string;
  readonly color: string;
  /** Avatar hash (ADR-0035), `null` without one. */
  readonly avatar: string | null;
}

export interface ChatWindowState {
  /** `none`: no camp is open in the main window. */
  readonly status: "loading" | "none" | "ready";
  readonly campId: string;
  readonly campName: string;
  readonly selfId: UserId;
  /** The camp's server is unreachable: nothing can be sent. */
  readonly offline: boolean;
  /** A moderator forbade you to speak, which covers writing (ADR-0044). */
  readonly silenced: boolean;
  /** The camp's text channels, in its order. */
  readonly chats: readonly ChannelInfo[];
  readonly activeId: string | null;
  readonly threads: Readonly<Record<string, ChatThread>>;
  /** Each chat's last message, from snapshots and events. */
  readonly latest: SeqByChat;
  /** What this device has read (ADR-0045). */
  readonly read: SeqByChat;
  readonly members: Readonly<Record<UserId, ChatMember>>;
  readonly draft: string;
  readonly photo: OutgoingPhoto | null;
  readonly sending: boolean;
  readonly error: string | null;
}

const EMPTY_THREAD: ChatThread = { messages: [], loading: false, complete: false, error: null, unreadAfter: null };

const message = (err: unknown) => (err instanceof Error ? err.message : String(err));

/**
 * The chat window (M5 step 3, ADR-0046): the open camp's chats, one at a time; the feed
 * is fetched by pages and kept whole as messages arrive — a number skipped means some
 * were missed, and they are fetched before they are shown. Rust keeps what is read;
 * this window says when you have seen the end of a chat.
 */
export class ChatController {
  readonly store: Store<ChatWindowState>;
  readonly #api: CampApi;
  readonly #storage: KeyValueStorage;
  readonly #avatars = new Map<string, Promise<Uint8Array>>();
  readonly #photos = new Map<string, Promise<Uint8Array>>();
  #unsubscribe: Array<() => void> = [];
  /** Bumped per context load: answers about a camp no longer shown are dropped. */
  #generation = 0;

  constructor(api: CampApi, storage: KeyValueStorage) {
    this.#api = api;
    this.#storage = storage;
    this.store = createStore<ChatWindowState>({
      status: "loading",
      campId: "",
      campName: "",
      selfId: "",
      offline: false,
      silenced: false,
      chats: [],
      activeId: null,
      threads: {},
      latest: {},
      read: {},
      members: {},
      draft: "",
      photo: null,
      sending: false,
      error: null,
    });
  }

  async start(): Promise<void> {
    if (this.#unsubscribe.length === 0) {
      this.#unsubscribe = [this.#api.onLive((live) => this.#onLive(live)), this.#api.onChat((u) => this.#onChat(u))];
    }
    await this.#load();
  }

  dispose(): void {
    for (const unsubscribe of this.#unsubscribe) unsubscribe();
    this.#unsubscribe = [];
  }

  /** Shows a chat, fetching its latest page the first time. */
  async select(chatId: string): Promise<void> {
    const { campId, chats, threads } = this.store.get();
    if (!chats.some((c) => c.id === chatId)) return;
    this.store.patch({ activeId: chatId, error: null });
    this.#remember(campId, chatId);
    if (!threads[chatId]) await this.#loadLatest(chatId);
  }

  /** The page before the oldest message held, when the feed is scrolled to its top. */
  async loadOlder(): Promise<void> {
    const { campId, activeId } = this.store.get();
    if (activeId === null) return;
    const thread = this.#thread(activeId);
    const first = thread.messages[0];
    if (thread.loading || thread.complete || first === undefined) return;
    this.#setThread(activeId, { ...thread, loading: true, error: null });
    const generation = this.#generation;
    try {
      const older = await this.#api.history(campId, activeId, first.seq, PAGE);
      if (generation !== this.#generation) return;
      const now = this.#thread(activeId);
      this.#setThread(activeId, {
        ...now,
        messages: mergeMessages(now.messages, older),
        loading: false,
        complete: older.length < PAGE || older[0]?.seq === 1,
      });
    } catch (err) {
      if (generation === this.#generation) this.#setThread(activeId, { ...this.#thread(activeId), loading: false, error: message(err) });
    }
  }

  /** The window shows the end of the active chat and has the focus: it is read. */
  markSeen(): void {
    const { campId, activeId, read } = this.store.get();
    if (activeId === null) return;
    const last = this.#thread(activeId).messages.at(-1)?.seq ?? 0;
    if (last <= (read[activeId] ?? 0)) return;
    this.store.patch({ read: advance(read, activeId, last) });
    void this.#api.markRead(campId, activeId, last).catch(() => {});
  }

  setDraft(draft: string): void {
    this.store.patch({ draft, error: null });
  }

  /** A problem worth saying in the composer (a photo that would not encode). */
  fail(reason: string): void {
    this.store.patch({ error: reason });
  }

  attach(photo: OutgoingPhoto): void {
    this.store.patch({ photo, error: null });
  }

  detach(): void {
    this.store.patch({ photo: null });
  }

  /** Whether the composer holds something the camp would take. */
  canSend(): boolean {
    const { draft, photo, sending, offline, silenced, activeId } = this.store.get();
    const text = draft.trimEnd();
    return (
      activeId !== null &&
      !sending &&
      !offline &&
      !silenced &&
      (text.trim().length > 0 || photo !== null) &&
      [...text].length <= MAX_MESSAGE_CHARS
    );
  }

  /** Sends the draft (and the photo) to the active chat; on a refusal both stay. */
  async send(): Promise<void> {
    if (!this.canSend()) return;
    const { campId, activeId, draft, photo } = this.store.get();
    if (activeId === null) return;
    this.store.patch({ sending: true, error: null });
    try {
      const sent = await this.#api.send(campId, activeId, draft.trimEnd(), photo);
      this.store.patch({ sending: false, draft: "", photo: null });
      this.#receive(sent);
    } catch (err) {
      this.store.patch({ sending: false, error: message(err) });
    }
  }

  /** A member's avatar by the snapshot's hash (once per hash; a failure is retried). */
  avatarOf(userId: string, hash: string): Promise<Uint8Array> {
    return this.#cached(this.#avatars, hash, () => this.#api.avatar(this.store.get().campId, userId, hash));
  }

  /** A photo of a message, by its hash. */
  photoOf(hash: string): Promise<Uint8Array> {
    return this.#cached(this.#photos, hash, () => this.#api.photo(this.store.get().campId, hash));
  }

  #cached(cache: Map<string, Promise<Uint8Array>>, hash: string, fetch: () => Promise<Uint8Array>) {
    let bytes = cache.get(hash);
    if (!bytes) {
      bytes = fetch();
      cache.set(hash, bytes);
      bytes.catch(() => cache.delete(hash));
    }
    return bytes;
  }

  // ---- the camp ----

  async #load(): Promise<void> {
    const generation = ++this.#generation;
    let context: ChatContext | null;
    try {
      context = await this.#api.chatContext();
    } catch (err) {
      this.store.patch({ status: "none", error: message(err) });
      return;
    }
    if (generation !== this.#generation) return;
    if (context === null) {
      this.store.patch({ status: "none", campId: "", chats: [], activeId: null, threads: {} });
      return;
    }
    const { camp, live } = context;
    this.#avatars.clear();
    this.#photos.clear();
    this.store.patch({
      status: "ready",
      campId: camp.id,
      campName: camp.name,
      selfId: camp.self.userId,
      offline: camp.offline,
      silenced: false,
      chats: camp.channels.filter((c) => c.kind === "text"),
      activeId: null,
      threads: {},
      latest: {},
      read: camp.read,
      members: { [camp.self.userId]: { name: camp.self.name, color: avatarColor(camp.self.userId), avatar: null } },
      draft: "",
      photo: null,
      error: null,
    });
    if (live?.state) this.#applyState(live.state, live.online);
    const { chats } = this.store.get();
    const remembered = this.#remembered(camp.id);
    const first = chats.find((c) => c.id === remembered) ?? chats[0];
    if (first) await this.select(first.id);
  }

  #onLive(live: CampLive): void {
    const { campId, status } = this.store.get();
    // The main window opened another camp: the chat goes with it.
    if (live.campId !== campId || status === "none") {
      void this.#load();
      return;
    }
    if (!live.state) {
      this.store.patch({ offline: !live.online });
      return;
    }
    this.#applyState(live.state, live.online);
    // Back online, or a message event missed: fetch what the snapshot says is there.
    const { threads, latest } = this.store.get();
    for (const [chatId, thread] of Object.entries(threads)) {
      const last = latest[chatId] ?? 0;
      if (thread.messages.length > 0 && gapBefore(thread.messages, last + 1) > 0) void this.#fill(chatId, last);
    }
  }

  #applyState(state: CampState, online: boolean): void {
    const { selfId, activeId, latest } = this.store.get();
    const chats = state.channels.filter((c) => c.kind === "text");
    const members: Record<UserId, ChatMember> = {};
    for (const m of state.members) members[m.userId] = { name: m.name, color: avatarColor(m.userId), avatar: m.avatar ?? null };
    let next = latest;
    for (const [chat, seq] of Object.entries(state.chats ?? {})) next = advance(next, chat, seq);
    const self = state.members.find((m) => m.userId === selfId);
    this.store.patch({
      campName: state.name,
      offline: !online,
      silenced: self?.silenced === true,
      chats,
      members,
      latest: next,
    });
    // The chat shown was removed: show another.
    if (activeId !== null && !chats.some((c) => c.id === activeId)) {
      const other = chats[0];
      if (other) void this.select(other.id);
      else this.store.patch({ activeId: null });
    }
  }

  #onChat(update: ChatUpdate): void {
    if (update.campId !== this.store.get().campId) return;
    if (update.type === "read") {
      this.store.patch({ read: advance(this.store.get().read, update.channelId, update.seq) });
      return;
    }
    this.#receive(update.message);
  }

  /** A message from the camp or our own send: added in place, or the gap before it filled. */
  #receive(incoming: ChatMessage): void {
    const { channelId, seq } = incoming;
    this.store.patch({ latest: advance(this.store.get().latest, channelId, seq) });
    const thread = this.store.get().threads[channelId];
    // A chat never opened here is fetched whole when it is.
    if (!thread || thread.messages.length === 0) return;
    if (gapBefore(thread.messages, seq) === 0) {
      this.#setThread(channelId, { ...thread, messages: mergeMessages(thread.messages, [incoming]) });
    } else {
      void this.#fill(channelId, seq);
    }
  }

  /** Fetches the messages after the last one held, up to `upTo`. */
  async #fill(chatId: string, upTo: number): Promise<void> {
    const { campId } = this.store.get();
    const generation = this.#generation;
    try {
      const fresh = await this.#api.history(campId, chatId, upTo + 1, PAGE);
      if (generation !== this.#generation) return;
      const thread = this.#thread(chatId);
      const joined = fresh.length > 0 && gapBefore(thread.messages, fresh[0]!.seq) === 0;
      // Too far behind to join up: the latest page stands alone, older ones load on scroll.
      this.#setThread(chatId, {
        ...thread,
        messages: joined ? mergeMessages(thread.messages, fresh) : fresh,
        complete: joined ? thread.complete : fresh[0]?.seq === 1,
      });
    } catch {
      // The next snapshot or message tries again.
    }
  }

  async #loadLatest(chatId: string): Promise<void> {
    const { campId, read } = this.store.get();
    this.#setThread(chatId, { ...EMPTY_THREAD, loading: true });
    const generation = this.#generation;
    try {
      const page = await this.#api.history(campId, chatId, null, PAGE);
      if (generation !== this.#generation) return;
      const last = page.at(-1)?.seq ?? 0;
      this.store.patch({ latest: advance(this.store.get().latest, chatId, last) });
      this.#setThread(chatId, {
        messages: page,
        loading: false,
        complete: page.length < PAGE || page[0]?.seq === 1,
        error: null,
        unreadAfter: read[chatId] ?? 0,
      });
    } catch (err) {
      if (generation === this.#generation) this.#setThread(chatId, { ...EMPTY_THREAD, error: message(err) });
    }
  }

  #thread(chatId: string): ChatThread {
    return this.store.get().threads[chatId] ?? EMPTY_THREAD;
  }

  #setThread(chatId: string, thread: ChatThread): void {
    this.store.patch({ threads: { ...this.store.get().threads, [chatId]: thread } });
  }

  #remembered(campId: string): string | null {
    try {
      const all = JSON.parse(this.#storage.getItem(LAST_CHAT_KEY) ?? "{}") as Record<string, string>;
      return all[campId] ?? null;
    } catch {
      return null;
    }
  }

  #remember(campId: string, chatId: string): void {
    let all: Record<string, string> = {};
    try {
      all = JSON.parse(this.#storage.getItem(LAST_CHAT_KEY) ?? "{}") as Record<string, string>;
    } catch {
      // A damaged entry is replaced.
    }
    this.#storage.setItem(LAST_CHAT_KEY, JSON.stringify({ ...all, [campId]: chatId }));
  }
}
