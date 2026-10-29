import { DEMO_CAMP, DEMO_SELF_ID } from "../domain/demo-camp.ts";
import type {
  CampApi,
  CampList,
  CampPreview,
  ChannelInfo,
  ChatContext,
  ChatMessage,
  ChatUpdate,
  OpenCamp,
  OutgoingPhoto,
} from "./camp-api.ts";

const DEMO_ID = "demo";
const DEMO_LINK = "ourcamp://192.0.2.15/join/7k2q9m4x8v1c3b5n?fp=" + "A".repeat(43);

/** The demo's chats (ADR-0044): text channels beside the campfires. */
const DEMO_CHATS: readonly ChannelInfo[] = [
  { id: "chat", name: "Общий", kind: "text", afk: false },
  { id: "flood", name: "Флуд", kind: "text", afk: false },
];

/** What was said in the demo chats: yesterday evening, then tonight (the mockup's lines). */
function demoMessages(): Map<string, ChatMessage[]> {
  const evening = new Date();
  evening.setHours(21, 0, 0, 0);
  const yesterday = evening.getTime() - 24 * 60 * 60 * 1000;
  // Tonight's lines were a few minutes ago, whenever the demo runs.
  const tonight = Date.now() - 10 * 60 * 1000;
  const at = (day: number, minutes: number) => day + minutes * 60 * 1000;
  const say = (channelId: string) => {
    let seq = 0;
    return (authorId: string, authorName: string, sentAt: number, text: string): ChatMessage => ({
      channelId,
      seq: ++seq,
      authorId,
      authorName,
      sentAt,
      text,
    });
  };
  const chat = say("chat");
  const flood = say("flood");
  return new Map([
    [
      "chat",
      [
        chat("artem", "Артём", at(yesterday, 100), "Всем спасибо за вечер, было огонь"),
        chat("olya", "Оля", at(yesterday, 101), "Завтра в то же время?"),
        chat("lena", "Лена", at(yesterday, 103), "Я смогу только после девяти"),
        chat("max", "Макс", at(yesterday, 104), "Тогда в 21:00, кто опоздает — дежурит по костру"),
        chat("artem", "Артём", at(yesterday, 105), "Договорились"),
        chat("artem", "Артём", at(tonight, 2), "Лена, включи стрим, хочу глянуть билд"),
        chat("lena", "Лена", at(tonight, 3), "Запустила, жмите LIVE возле моего ника"),
        chat("max", "Макс", at(tonight, 5), "Я без микро, пишу сюда. Через 10 минут буду"),
      ],
    ],
    [
      "flood",
      [
        flood("olya", "Оля", at(yesterday, -140), "Кто в выходные за город? Нужна ещё одна машина"),
        flood("kirill", "Кирилл", at(yesterday, -128), "Я на машине, место есть на троих"),
      ],
    ],
  ]);
}

/**
 * Browser-only stand-in for the camp commands (`pnpm --filter @ourcamp/desktop dev`):
 * starts with no camps so the first-run screens can be designed, and any well-formed
 * `ourcamp://` link "joins" the demo camp.
 */
export class DemoCampApi implements CampApi {
  #joined = false;
  #name = "";
  readonly #messages = demoMessages();
  /** Read up to yesterday in "Общий": tonight's lines and the one to come are new. */
  #read: Record<string, number> = { chat: 5, flood: 2 };
  #chat: ((update: ChatUpdate) => void) | null = null;
  #wrote = false;

  list(): Promise<CampList> {
    return Promise.resolve({
      displayName: this.#name,
      camps: this.#joined ? [{ id: DEMO_ID, name: DEMO_CAMP.name, role: "admin" as const }] : [],
      current: this.#joined ? DEMO_ID : null,
    });
  }

  async preview(link: string): Promise<CampPreview> {
    await delay(300);
    if (!link.trim().toLowerCase().startsWith("ourcamp://")) throw new Error("Это не ссылка OurCamp");
    return { name: DEMO_CAMP.name, channels: DEMO_CAMP.channels.length, kind: link.includes("/claim/") ? "claim" : link.includes("/join/") ? "join" : "open", usable: !link.includes("/claim/") };
  }

  async join(_link: string, name: string): Promise<OpenCamp> {
    await delay(300);
    this.#joined = true;
    this.#name = name.trim();
    return this.#open();
  }

  open(): Promise<OpenCamp> {
    return Promise.resolve(this.#open());
  }

  leave(): Promise<boolean> {
    this.#joined = false;
    return Promise.resolve(true);
  }

  invite(): Promise<string> {
    return Promise.resolve(DEMO_LINK);
  }

  moveMember(): Promise<void> {
    // The demo's people are pictures: nobody's app is there to follow the move.
    return Promise.resolve();
  }

  #avatar: Uint8Array | null = null;

  profileAvatar(): Promise<Uint8Array | null> {
    return Promise.resolve(this.#avatar);
  }

  setProfileAvatar(picture: Uint8Array | null): Promise<void> {
    this.#avatar = picture;
    return Promise.resolve();
  }

  avatar(): Promise<Uint8Array> {
    // The demo's people have no pictures; they show their initials.
    return Promise.reject(new Error("no avatar in the demo"));
  }

  kick(): Promise<void> {
    return Promise.resolve();
  }

  setSilenced(): Promise<void> {
    return Promise.resolve();
  }

  onLive(): () => void {
    // The demo camp brings its people with it (see `main.tsx`); nothing goes live.
    return () => {};
  }

  onDirective(): () => void {
    // Nobody moderates the demo camp.
    return () => {};
  }

  history(_id: string, channelId: string, before: number | null, limit: number): Promise<ChatMessage[]> {
    const all = this.#messages.get(channelId) ?? [];
    const older = all.filter((m) => before === null || m.seq < before);
    return Promise.resolve(older.slice(-limit));
  }

  async send(_id: string, channelId: string, text: string, photo: OutgoingPhoto | null): Promise<ChatMessage> {
    await delay(150);
    const all = this.#messages.get(channelId) ?? [];
    const message: ChatMessage = {
      channelId,
      seq: (all.at(-1)?.seq ?? 0) + 1,
      authorId: DEMO_SELF_ID,
      authorName: this.#name || "Дима",
      sentAt: Date.now(),
      text: text.trimEnd(),
      ...(photo && { photo: { hash: this.#keep(photo.bytes), width: photo.width, height: photo.height } }),
    };
    this.#messages.set(channelId, [...all, message]);
    this.#chat?.({ type: "message", campId: DEMO_ID, message });
    return message;
  }

  /** Photos sent in the demo, kept for this page's life. */
  readonly #photos = new Map<string, Uint8Array>();

  #keep(bytes: Uint8Array): string {
    const hash = `demo-${this.#photos.size}`;
    this.#photos.set(hash, bytes);
    return hash;
  }

  photo(_id: string, hash: string): Promise<Uint8Array> {
    const bytes = this.#photos.get(hash);
    return bytes ? Promise.resolve(bytes) : Promise.reject(new Error("no such photo in the demo"));
  }

  markRead(_id: string, channelId: string, seq: number): Promise<void> {
    if ((this.#read[channelId] ?? 0) < seq) {
      this.#read = { ...this.#read, [channelId]: seq };
      this.#chat?.({ type: "read", campId: DEMO_ID, channelId, seq });
    }
    return Promise.resolve();
  }

  onChat(listener: (update: ChatUpdate) => void): () => void {
    this.#chat = listener;
    return () => (this.#chat = null);
  }

  /**
   * The demo has no live snapshots (its people are pictures), so the badge learns of new
   * messages the other way: someone writes a moment after the camp opens, once.
   */
  #someoneWrites(): void {
    if (this.#wrote) return;
    this.#wrote = true;
    setTimeout(() => {
      const all = this.#messages.get("chat") ?? [];
      const message: ChatMessage = {
        channelId: "chat",
        seq: (all.at(-1)?.seq ?? 0) + 1,
        authorId: "lena",
        authorName: "Лена",
        sentAt: Date.now(),
        text: "Кто со мной в «Катку»?",
      };
      this.#messages.set("chat", [...all, message]);
      this.#chat?.({ type: "message", campId: DEMO_ID, message });
    }, 3000);
  }

  /**
   * The demo chat window opens on its own page: it brings the demo camp along, as if the
   * main window had it open, with the demo's people.
   */
  chatContext(): Promise<ChatContext | null> {
    const open = this.#open();
    const members = Object.values(DEMO_CAMP.members).map((m) => ({
      userId: m.id,
      name: m.name,
      role: m.role,
      silenced: m.silenced,
    }));
    return Promise.resolve({
      camp: open,
      live: {
        campId: DEMO_ID,
        online: true,
        state: { name: DEMO_CAMP.name, channels: open.channels, members, presence: [], chats: this.#latest() },
      },
    });
  }

  #latest(): Record<string, number> {
    return Object.fromEntries([...this.#messages].map(([id, all]) => [id, all.at(-1)?.seq ?? 0]));
  }

  #open(): OpenCamp {
    this.#someoneWrites();
    return {
      id: DEMO_ID,
      name: DEMO_CAMP.name,
      channels: [
        ...DEMO_CAMP.channels.map((c) => ({ id: c.id, name: c.name, kind: "voice" as const, afk: c.afk })),
        ...DEMO_CHATS,
      ],
      self: { userId: DEMO_SELF_ID, name: this.#name || "Дима", role: "admin" as const },
      offline: false,
      read: this.#read,
    };
  }
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
