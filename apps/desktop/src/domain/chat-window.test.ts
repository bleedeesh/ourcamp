import { test } from "node:test";
import assert from "node:assert/strict";
import type {
  CampApi,
  CampLive,
  CampState,
  ChannelInfo,
  ChatContext,
  ChatMessage,
  ChatUpdate,
  OpenCamp,
  OutgoingPhoto,
} from "../engine/camp-api.ts";
import { ChatController, PAGE } from "./chat-window.ts";
import { MemoryStorage } from "./test-support.ts";

const CHANNELS: ChannelInfo[] = [
  { id: "c0", name: "Voice", kind: "voice", afk: false },
  { id: "t1", name: "Общий", kind: "text", afk: false },
  { id: "t2", name: "Флуд", kind: "text", afk: false },
];

const msg = (channelId: string, seq: number, authorId = "lena"): ChatMessage => ({
  channelId,
  seq,
  authorId,
  authorName: authorId,
  sentAt: seq * 1000,
  text: `№${seq}`,
});

/** A camp server in memory: chats with messages 1…n, history like the real one. */
class FakeChatApi implements Pick<CampApi, "chatContext" | "history" | "send" | "markRead" | "onLive" | "onChat" | "avatar" | "photo"> {
  calls: string[] = [];
  chats = new Map<string, ChatMessage[]>([
    ["t1", Array.from({ length: 120 }, (_, i) => msg("t1", i + 1))],
    ["t2", [msg("t2", 1)]],
  ]);
  campId = "vk";
  read: Record<string, number> = { t1: 118 };
  silenced = false;
  failSend: string | null = null;

  state(): CampState {
    return {
      name: "Вечерний кооп",
      channels: CHANNELS,
      members: [
        { userId: "me", name: "Дима", role: "member", silenced: this.silenced },
        { userId: "lena", name: "Лена", role: "member", silenced: false, avatar: "AAAAAAAAAAAAAAAA" },
      ],
      presence: [],
      chats: Object.fromEntries([...this.chats].map(([id, all]) => [id, all.at(-1)?.seq ?? 0])),
    };
  }

  chatContext(): Promise<ChatContext | null> {
    const camp: OpenCamp = {
      id: this.campId,
      name: "Вечерний кооп",
      channels: CHANNELS,
      self: { userId: "me", name: "Дима", role: "member" },
      offline: false,
      read: this.read,
    };
    return Promise.resolve({ camp, live: { campId: this.campId, online: true, state: this.state() } });
  }

  history(_id: string, chatId: string, before: number | null, limit: number): Promise<ChatMessage[]> {
    this.calls.push(`history:${chatId}:${before}`);
    const all = this.chats.get(chatId) ?? [];
    return Promise.resolve(all.filter((m) => before === null || m.seq < before).slice(-limit));
  }

  send(_id: string, chatId: string, text: string, photo: OutgoingPhoto | null): Promise<ChatMessage> {
    this.calls.push(`send:${chatId}:${text}:${photo ? "photo" : "-"}`);
    if (this.failSend) return Promise.reject(new Error(this.failSend));
    return Promise.resolve(this.add(chatId, "me"));
  }

  markRead(_id: string, chatId: string, seq: number): Promise<void> {
    this.calls.push(`read:${chatId}:${seq}`);
    return Promise.resolve();
  }

  /** Someone writes: kept on the "server"; `silent` leaves out the event (a missed one). */
  add(chatId: string, authorId: string, silent = true): ChatMessage {
    const all = this.chats.get(chatId) ?? [];
    const m = msg(chatId, (all.at(-1)?.seq ?? 0) + 1, authorId);
    this.chats.set(chatId, [...all, m]);
    if (!silent) this.emitChat({ type: "message", campId: this.campId, message: m });
    return m;
  }

  #live: ((l: CampLive) => void) | null = null;
  onLive(listener: (l: CampLive) => void) {
    this.#live = listener;
    return () => (this.#live = null);
  }
  emitLive(live: CampLive) {
    this.#live?.(live);
  }
  #chat: ((u: ChatUpdate) => void) | null = null;
  onChat(listener: (u: ChatUpdate) => void) {
    this.#chat = listener;
    return () => (this.#chat = null);
  }
  emitChat(u: ChatUpdate) {
    this.#chat?.(u);
  }
  avatar(): Promise<Uint8Array> {
    return Promise.resolve(Uint8Array.of(1));
  }
  photo(): Promise<Uint8Array> {
    return Promise.resolve(Uint8Array.of(2));
  }
}

const tick = () => new Promise((r) => setTimeout(r, 0));

async function setup(storage = new MemoryStorage()) {
  const api = new FakeChatApi();
  const chat = new ChatController(api as unknown as CampApi, storage);
  await chat.start();
  return { api, chat, storage };
}

const seqs = (chat: ChatController, chatId: string) =>
  chat.store.get().threads[chatId]?.messages.map((m) => m.seq) ?? [];

test("opening shows the camp's first chat with its latest page and where the unread start", async () => {
  const { chat } = await setup();
  const s = chat.store.get();
  assert.equal(s.status, "ready");
  assert.deepEqual(s.chats.map((c) => c.id), ["t1", "t2"], "text channels only");
  assert.equal(s.activeId, "t1");
  const thread = s.threads["t1"]!;
  assert.equal(thread.messages.length, PAGE);
  assert.equal(thread.messages.at(-1)?.seq, 120);
  assert.equal(thread.complete, false);
  assert.equal(thread.unreadAfter, 118);
  assert.equal(s.members["lena"]?.avatar, "AAAAAAAAAAAAAAAA");
});

test("scrolling up fetches older pages until the first message", async () => {
  const { chat } = await setup();
  await chat.loadOlder();
  assert.equal(seqs(chat, "t1")[0], 21);
  await chat.loadOlder();
  assert.equal(seqs(chat, "t1")[0], 1);
  assert.equal(chat.store.get().threads["t1"]?.complete, true);
  assert.deepEqual(seqs(chat, "t1"), Array.from({ length: 120 }, (_, i) => i + 1), "whole, in order, each once");
});

test("a message in place is added; a skipped number makes the window fetch the gap", async () => {
  const { api, chat } = await setup();
  api.add("t1", "lena", false);
  assert.equal(seqs(chat, "t1").at(-1), 121, "added from the event alone");
  const before = api.calls.length;

  api.add("t1", "lena"); // 122: its event is lost
  api.add("t1", "max", false); // 123 arrives
  await tick();
  assert.ok(api.calls.slice(before).includes("history:t1:124"), "fetches up to 123");
  assert.deepEqual(seqs(chat, "t1").slice(-3), [121, 122, 123]);

  api.add("t2", "lena", false);
  assert.deepEqual(seqs(chat, "t2"), [], "a chat not opened yet is fetched when it is");
  assert.equal(chat.store.get().latest["t2"], 2, "but its count moves");
});

test("a snapshot after an outage fills what was missed", async () => {
  const { api, chat } = await setup();
  api.add("t1", "lena");
  api.add("t1", "lena");
  api.emitLive({ campId: "vk", online: true, state: api.state() });
  await tick();
  assert.deepEqual(seqs(chat, "t1").slice(-2), [121, 122]);
});

test("seen at the end, a chat is read once; read marks from elsewhere are kept", async () => {
  const { api, chat } = await setup();
  chat.markSeen();
  chat.markSeen();
  assert.deepEqual(api.calls.filter((c) => c.startsWith("read")), ["read:t1:120"]);
  api.emitChat({ type: "read", campId: "vk", channelId: "t2", seq: 1 });
  assert.equal(chat.store.get().read["t2"], 1);
});

test("sending clears the composer; a refusal keeps it with the reason", async () => {
  const { api, chat } = await setup();
  assert.equal(chat.canSend(), false, "nothing to send");
  chat.setDraft("Привет  \n");
  assert.equal(chat.canSend(), true);
  await chat.send();
  assert.ok(api.calls.includes("send:t1:Привет:-"), "trailing whitespace trimmed");
  assert.equal(chat.store.get().draft, "");
  assert.equal(seqs(chat, "t1").at(-1), 121, "shown at once, before its event");

  api.failSend = "Вам запрещено говорить и писать на этом сервере";
  chat.setDraft("Ещё");
  chat.attach({ bytes: Uint8Array.of(1), width: 10, height: 10 });
  await chat.send();
  const s = chat.store.get();
  assert.equal(s.draft, "Ещё");
  assert.notEqual(s.photo, null);
  assert.match(s.error ?? "", /запрещено/);

  chat.setDraft("я".repeat(2001));
  assert.equal(chat.canSend(), false, "over the limit");
});

test("a silenced member cannot write; the main window switching camps takes the chat along", async () => {
  const { api, chat } = await setup();
  api.silenced = true;
  api.emitLive({ campId: "vk", online: true, state: api.state() });
  chat.setDraft("x");
  assert.equal(chat.store.get().silenced, true);
  assert.equal(chat.canSend(), false);

  api.campId = "other";
  api.emitLive({ campId: "other", online: true, state: api.state() });
  await tick();
  await tick();
  assert.equal(chat.store.get().campId, "other");
  assert.equal(chat.store.get().draft, "", "a fresh composer");
});

test("the window opens the chat shown last in this camp", async () => {
  const storage = new MemoryStorage();
  const first = await setup(storage);
  await first.chat.select("t2");
  const again = await setup(storage);
  assert.equal(again.chat.store.get().activeId, "t2");
});
