import { test } from "node:test";
import assert from "node:assert/strict";
import type {
  CampApi,
  CampDirective,
  CampList,
  CampLive,
  CampPreview,
  ChatMessage,
  ChatUpdate,
  OpenCamp,
} from "../engine/camp-api.ts";
import { campfires, CampsController, campShort, NO_CAMP } from "./camps.ts";
import { FakeEngine, MemoryStorage } from "./test-support.ts";
import { VoiceController } from "./voice.ts";
import { VolumeBook } from "./volume.ts";

const FP = "A".repeat(43);
const LINK = `ourcamp://192.0.2.15/join/7k2q9m4x8v1c3b5n?fp=${FP}`;
const CLAIM = `ourcamp://192.0.2.15/claim/bcmqj75a9v64svgfthvb6fq4mw?fp=${FP}`;
const OPEN = `ourcamp://ourcamp.kz?fp=${FP}`;

const opened = (id: string, name: string, offline = false): OpenCamp => ({
  id,
  name,
  channels: [
    { id: "c0", name: "Лобби", kind: "voice", afk: false },
    { id: "c1", name: "Катка", kind: "voice", afk: false },
  ],
  self: { userId: "me", name: "Дима", role: id === "own" ? "admin" : "member" },
  offline,
  read: {},
});

/** Records calls; previews resolve when the test says so. */
class FakeCampApi implements CampApi {
  calls: string[] = [];
  list_: CampList = { displayName: "", camps: [], current: null };
  pending = new Map<string, { resolve: (p: CampPreview) => void; reject: (e: Error) => void }>();
  failOpen: string | null = null;
  failJoin: string | null = null;

  list(): Promise<CampList> {
    return Promise.resolve(this.list_);
  }
  preview(link: string): Promise<CampPreview> {
    this.calls.push(`preview:${link.slice(0, 20)}`);
    return new Promise((resolve, reject) => this.pending.set(link, { resolve, reject }));
  }
  join(link: string, name: string): Promise<OpenCamp> {
    this.calls.push(`join:${name}`);
    if (this.failJoin) return Promise.reject(new Error(this.failJoin));
    const id = link.includes("/claim/") ? "own" : "vk";
    this.list_ = { displayName: name, camps: [{ id, name: "Вечерний кооп", role: "member" }], current: id };
    return Promise.resolve(opened(id, "Вечерний кооп"));
  }
  open(id: string): Promise<OpenCamp> {
    this.calls.push(`open:${id}`);
    if (this.failOpen) return Promise.reject(new Error(this.failOpen));
    return Promise.resolve(opened(id, id === "vk" ? "Вечерний кооп" : "Свой"));
  }
  /** What `leave` answers: the camp heard it (`true`) or was unreachable. */
  leaveTold = true;
  leave(id: string): Promise<boolean> {
    this.calls.push(`leave:${id}`);
    const camps = this.list_.camps.filter((c) => c.id !== id);
    this.list_ = { ...this.list_, camps, current: this.list_.current === id ? null : this.list_.current };
    return Promise.resolve(this.leaveTold);
  }
  failMove: string | null = null;
  moveMember(id: string, userId: string, channelId: string): Promise<void> {
    this.calls.push(`move:${id}:${userId}:${channelId}`);
    return this.failMove ? Promise.reject(new Error(this.failMove)) : Promise.resolve();
  }
  avatarFetches = 0;
  profile: Uint8Array | null = null;
  profileAvatar(): Promise<Uint8Array | null> {
    return Promise.resolve(this.profile);
  }
  setProfileAvatar(picture: Uint8Array | null): Promise<void> {
    this.calls.push(`avatar:${picture === null ? "none" : picture.length}`);
    this.profile = picture;
    return Promise.resolve();
  }
  avatar(_id: string, _userId: string, hash: string): Promise<Uint8Array> {
    this.avatarFetches++;
    return hash === "bad" ? Promise.reject(new Error("gone")) : Promise.resolve(Uint8Array.of(1, 2, 3));
  }
  failModeration: string | null = null;
  kick(id: string, userId: string): Promise<void> {
    this.calls.push(`kick:${id}:${userId}`);
    return this.failModeration ? Promise.reject(new Error(this.failModeration)) : Promise.resolve();
  }
  setSilenced(id: string, userId: string, silenced: boolean): Promise<void> {
    this.calls.push(`silence:${id}:${userId}:${silenced}`);
    return this.failModeration ? Promise.reject(new Error(this.failModeration)) : Promise.resolve();
  }
  invite(id: string): Promise<string> {
    this.calls.push(`invite:${id}`);
    return Promise.resolve(LINK);
  }
  #live: ((live: CampLive) => void) | null = null;
  onLive(listener: (live: CampLive) => void) {
    this.#live = listener;
    return () => (this.#live = null);
  }
  emitLive(live: CampLive) {
    this.#live?.(live);
  }
  #directive: ((d: CampDirective) => void) | null = null;
  onDirective(listener: (d: CampDirective) => void) {
    this.#directive = listener;
    return () => (this.#directive = null);
  }
  history(): Promise<ChatMessage[]> {
    return Promise.resolve([]);
  }
  send(): Promise<ChatMessage> {
    return Promise.reject(new Error("not in these tests"));
  }
  photo(): Promise<Uint8Array> {
    return Promise.reject(new Error("not in these tests"));
  }
  markRead(id: string, channelId: string, seq: number): Promise<void> {
    this.calls.push(`read:${id}:${channelId}:${seq}`);
    return Promise.resolve();
  }
  #chat: ((u: ChatUpdate) => void) | null = null;
  onChat(listener: (u: ChatUpdate) => void) {
    this.#chat = listener;
    return () => (this.#chat = null);
  }
  emitChat(u: ChatUpdate) {
    this.#chat?.(u);
  }
  /** Like Rust: on a kick the device has already forgotten the camp. */
  emitDirective(d: CampDirective) {
    if (d.type === "kicked") {
      const camps = this.list_.camps.filter((c) => c.id !== d.campId);
      this.list_ = { ...this.list_, camps, current: this.list_.current === d.campId ? null : this.list_.current };
    }
    this.#directive?.(d);
  }
}

function setup() {
  const engine = new FakeEngine();
  const voice = new VoiceController(engine, new VolumeBook(new MemoryStorage()), NO_CAMP, "");
  voice.start();
  const api = new FakeCampApi();
  const copied: string[] = [];
  const copy = (text: string) => {
    copied.push(text);
    return Promise.resolve();
  };
  const camps = new CampsController(api, voice, { copy });
  return { engine, voice, api, camps, copied };
}

const tick = () => new Promise((r) => setTimeout(r, 0));

test("first run without camps shows the empty screen", async () => {
  const { camps } = setup();
  assert.equal(camps.store.get().view, "loading");
  await camps.start();
  assert.equal(camps.store.get().view, "empty");
});

test("start reopens the last camp and fills the tree", async () => {
  const { api, camps, voice } = setup();
  api.list_ = { displayName: "Дима", camps: [{ id: "vk", name: "Вечерний кооп", role: "member" }], current: "vk" };
  await camps.start();
  assert.equal(camps.store.get().view, "camp");
  const camp = voice.store.get().camp;
  assert.equal(camp.name, "Вечерний кооп");
  assert.equal(camp.short, "ВК");
  assert.deepEqual(
    camp.channels.map((c) => c.id),
    ["c0", "c1"],
  );
  assert.equal(voice.store.get().selfId, "me");
  assert.equal(camp.members["me"]?.name, "Дима");
});

test("links are checked only once they could be complete; junk is explained", async () => {
  const { api, camps } = setup();
  await camps.setLink("ourcamp://192.0.2.15/jo");
  assert.equal(camps.store.get().check.state, "idle", "still typing");
  for (const partial of [
    "ourcamp://192.0.2.15",
    "ourcamp://ourcamp.k",
    "ourcamp://ourcamp.kz/join/7k2q9m4x",
    "ourcamp://localhost",
  ]) {
    await camps.setLink(partial);
    assert.equal(camps.store.get().check.state, "idle", partial);
  }
  assert.equal(api.calls.length, 0);
  await camps.setLink("https://example.com");
  const check = camps.store.get().check;
  assert.equal(check.state, "error");
  assert.match(check.state === "error" ? check.message : "", /ourcamp:\/\//);
});

test("a domain's link needs no fingerprint", async () => {
  const { api, camps } = setup();
  for (const link of ["ourcamp://OurCamp.kz", "ourcamp://ourcamp.kz:7781/join/7k2q9m4x8v1c3b5n/"]) {
    const pending = camps.setLink(link);
    assert.equal(camps.store.get().check.state, "checking", link);
    api.pending.get(link)?.resolve({ name: "OurCamp KZ", channels: 2, kind: "open", usable: true });
    await pending;
  }
  assert.equal(api.calls.length, 2);
});

test("a slow answer about an old link does not overwrite the current one", async () => {
  const { api, camps } = setup();
  const first = camps.setLink(LINK);
  const second = camps.setLink(CLAIM);
  api.pending.get(CLAIM)?.resolve({ name: "Свой", channels: 3, kind: "claim", usable: true });
  await second;
  api.pending.get(LINK)?.resolve({ name: "Старый", channels: 1, kind: "join", usable: true });
  await first;
  const check = camps.store.get().check;
  assert.equal(check.state === "ok" ? check.preview.name : "", "Свой");
});

test("joining needs a good link, a usable key and a name; then the camp opens", async () => {
  const { api, camps, voice } = setup();
  await camps.start();
  camps.openJoin();
  const pending = camps.setLink(CLAIM);
  api.pending.get(CLAIM)?.resolve({ name: "Свой", channels: 3, kind: "claim", usable: false });
  await pending;
  camps.setName("Дима");
  assert.equal(camps.canJoin(), false, "used admin key");

  const again = camps.setLink(LINK);
  api.pending.get(LINK)?.resolve({ name: "Вечерний кооп", channels: 3, kind: "join", usable: true });
  await again;
  camps.setName("   ");
  assert.equal(camps.canJoin(), false, "blank name");
  camps.setName("Дима");
  assert.equal(camps.canJoin(), true);

  await camps.join();
  const s = camps.store.get();
  assert.equal(s.view, "camp");
  assert.equal(s.currentId, "vk");
  assert.equal(s.displayName, "Дима");
  assert.equal(voice.store.get().camp.name, "Вечерний кооп");
  assert.ok(api.calls.includes("join:Дима"));
});

test("a refused join keeps the form open with the reason", async () => {
  const { api, camps } = setup();
  camps.openJoin();
  const pending = camps.setLink(LINK);
  api.pending.get(LINK)?.resolve({ name: "x", channels: 1, kind: "join", usable: true });
  await pending;
  camps.setName("Дима");
  api.failJoin = "Приглашение недействительно";
  await camps.join();
  const s = camps.store.get();
  assert.equal(s.view, "join");
  assert.equal(s.joinError, "Приглашение недействительно");
  assert.equal(s.joining, false);
});

test("switching camps leaves the current campfire first", async () => {
  const { api, camps, engine, voice } = setup();
  api.list_ = {
    displayName: "Дима",
    camps: [
      { id: "vk", name: "Вечерний кооп", role: "member" },
      { id: "own", name: "Свой", role: "admin" },
    ],
    current: "vk",
  };
  await camps.start();
  await voice.join("c0");
  await camps.switchTo("own");
  assert.equal(voice.store.get().channelId, null);
  assert.ok(engine.calls.includes("leave"));
  assert.equal(voice.store.get().camp.name, "Свой");
  assert.equal(camps.store.get().currentId, "own");

  await camps.switchTo("own");
  assert.equal(api.calls.filter((c) => c === "open:own").length, 1, "re-picking the current camp is a no-op");
});

test("an unreachable camp still opens, with a warning", async () => {
  const { api, camps } = setup();
  api.list_ = { displayName: "Дима", camps: [{ id: "vk", name: "Вечерний кооп", role: "member" }], current: "vk" };
  api.open = (id) => Promise.resolve(opened(id, "Вечерний кооп", true));
  await camps.start();
  const s = camps.store.get();
  assert.equal(s.view, "camp");
  assert.equal(s.offline, true);
  assert.match(s.error ?? "", /недоступен/);
});

test("copy invite puts the camp's link on the clipboard and says so", async () => {
  const { api, camps, copied } = setup();
  api.list_ = { displayName: "Дима", camps: [{ id: "vk", name: "Вечерний кооп", role: "member" }], current: "vk" };
  await camps.start();
  camps.toggleSwitcher();
  await camps.copyInvite();
  await tick();
  assert.deepEqual(copied, [LINK]);
  assert.equal(camps.store.get().switcherOpen, false);
  assert.match(camps.store.get().notice ?? "", /скопирована/);
});

test("cancel returns where you came from", async () => {
  const { camps } = setup();
  await camps.start();
  camps.openJoin();
  camps.cancelJoin();
  assert.equal(camps.store.get().view, "empty");
});

test("badges and counts read naturally", () => {
  assert.equal(campShort("Вечерний кооп"), "ВК");
  assert.equal(campShort("сервер"), "СЕ");
  assert.equal(campShort("  "), "?");
  assert.equal(campfires(1), "1 канал");
  assert.equal(campfires(3), "3 канала");
  assert.equal(campfires(5), "5 каналов");
  assert.equal(campfires(11), "11 каналов");
  assert.equal(campfires(22), "22 канала");
});

test("live state of the open camp reaches the tree; other camps and outages are handled", async () => {
  const { api, camps, voice } = setup();
  api.list_ = { displayName: "Дима", camps: [{ id: "vk", name: "Вечерний кооп", role: "member" }], current: "vk" };
  await camps.start();
  const live = (campId: string, online: boolean, withBob: boolean, name = "Вечерний кооп"): CampLive => ({
    campId,
    online,
    state: online
      ? {
          name,
          channels: opened("vk", name).channels,
          members: [{ userId: "bob", name: "Боб", role: "admin", silenced: false }],
          presence: withBob ? [{ channelId: "c1", members: [{ userId: "bob", micMuted: true }] }] : [],
        }
      : null,
  });

  api.emitLive(live("other-camp", true, true));
  assert.deepEqual(voice.store.get().camp.channels[1]?.memberIds, [], "not the open camp");

  api.emitLive(live("vk", true, true));
  const camp = voice.store.get().camp;
  assert.deepEqual(camp.channels[1]?.memberIds, ["bob"]);
  assert.equal(camp.members["bob"]?.role, "admin");
  assert.equal(camps.store.get().offline, false);

  api.emitLive(live("vk", false, false));
  assert.equal(camps.store.get().offline, true);
  assert.deepEqual(voice.store.get().camp.channels[1]?.memberIds, ["bob"], "last known state kept while offline");

  api.emitLive(live("vk", true, true, "Кооп на выходных"));
  assert.equal(voice.store.get().camp.name, "Кооп на выходных", "a rename reaches the header");
  assert.equal(camps.store.get().camps[0]?.name, "Кооп на выходных", "and the switcher");
});

test("leaving asks first, leaves voice, then opens the next camp", async () => {
  const { api, camps, engine, voice } = setup();
  api.list_ = {
    displayName: "Дима",
    camps: [
      { id: "vk", name: "Вечерний кооп", role: "member" },
      { id: "own", name: "Свой", role: "admin" },
    ],
    current: "vk",
  };
  await camps.start();
  await voice.join("c0");
  camps.toggleSwitcher();
  camps.askLeave();
  assert.equal(camps.store.get().leaveAsk, true);
  camps.cancelLeave();
  assert.equal(camps.store.get().leaveAsk, false, "changed their mind");
  assert.equal(camps.store.get().currentId, "vk");

  camps.askLeave();
  await camps.leave();
  assert.ok(engine.calls.includes("leave"), "out of the campfire first");
  assert.ok(api.calls.includes("leave:vk"));
  const state = camps.store.get();
  assert.deepEqual(
    state.camps.map((c) => c.id),
    ["own"],
  );
  assert.equal(state.currentId, "own", "the next camp opens");
  assert.equal(state.notice, "Вы покинули сервер «Вечерний кооп»");
  assert.equal(state.switcherOpen || state.leaveAsk, false);
});

test("leaving the last camp shows the first-run screen; an unreachable camp is said so", async () => {
  const { api, camps } = setup();
  api.list_ = { displayName: "Дима", camps: [{ id: "vk", name: "Вечерний кооп", role: "member" }], current: "vk" };
  await camps.start();
  api.leaveTold = false;
  await camps.leave();
  const state = camps.store.get();
  assert.equal(state.view, "empty");
  assert.equal(state.currentId, null);
  assert.match(state.notice ?? "", /убран из списка.*числитесь участником/);
});

/** Lets a chain of awaits inside the controller run out. */
const settle = async () => {
  for (let i = 0; i < 10; i++) await tick();
};

test("a moderator's move takes you to the channel; out of voice it does nothing", async () => {
  const { api, camps, engine, voice } = setup();
  api.list_ = { displayName: "Дима", camps: [{ id: "vk", name: "Вечерний кооп", role: "member" }], current: "vk" };
  await camps.start();
  api.emitDirective({ type: "moved", campId: "vk", channelId: "c1" });
  await settle();
  assert.equal(voice.store.get().channelId, null, "only someone in voice is moved");

  await voice.join("c0");
  api.emitDirective({ type: "moved", campId: "other", channelId: "c1" });
  await settle();
  assert.equal(voice.store.get().channelId, "c0", "another camp's word is not ours");

  api.emitDirective({ type: "moved", campId: "vk", channelId: "c1" });
  await settle();
  assert.equal(voice.store.get().channelId, "c1");
  assert.deepEqual(engine.calls.slice(-1), ["join:c1"]);
  assert.equal(camps.store.get().notice, "Вас перенесли в канал «Катка»");
});

test("a kick takes you out of voice and the camp, and says so", async () => {
  const { api, camps, engine, voice } = setup();
  api.list_ = {
    displayName: "Дима",
    camps: [
      { id: "vk", name: "Вечерний кооп", role: "member" },
      { id: "own", name: "Свой", role: "admin" },
    ],
    current: "vk",
  };
  await camps.start();
  await voice.join("c0");
  engine.disconnect("Вас убрали из канала");
  api.emitDirective({ type: "kicked", campId: "vk", campName: "Вечерний кооп" });
  await settle();
  const state = camps.store.get();
  assert.deepEqual(
    state.camps.map((c) => c.id),
    ["own"],
  );
  assert.equal(state.currentId, "own", "the next camp opens");
  assert.equal(voice.store.get().channelId, null);
  assert.equal(voice.store.get().error, null, "the kick notice explains it, not a bare disconnect");
  assert.match(state.notice ?? "", /^Вас выгнали с сервера «Вечерний кооп»/);
  assert.ok(!api.calls.includes("leave:vk"), "nothing to tell a camp that threw us out");

  api.emitDirective({ type: "kicked", campId: "own", campName: "Свой" });
  await settle();
  assert.equal(camps.store.get().view, "empty", "no camps left");
});

test("dragging yourself joins the channel; dragging someone else asks the camp", async () => {
  const { api, camps, engine, voice } = setup();
  api.list_ = { displayName: "Дима", camps: [{ id: "vk", name: "Вечерний кооп", role: "member" }], current: "vk" };
  await camps.start();
  await voice.join("c0");
  await camps.moveMember("me", "c1");
  assert.equal(voice.store.get().channelId, "c1");
  assert.deepEqual(engine.calls.slice(-1), ["join:c1"]);

  await camps.moveMember("bob", "c0");
  assert.deepEqual(api.calls.slice(-1), ["move:vk:bob:c0"]);
  api.failMove = "Для этого нужны права админа или модератора";
  await camps.moveMember("bob", "c1");
  assert.equal(camps.store.get().error, "Для этого нужны права админа или модератора");
});

test("kicking and silencing go to the camp; a refusal is said", async () => {
  const { api, camps, voice } = setup();
  api.list_ = { displayName: "Дима", camps: [{ id: "vk", name: "Вечерний кооп", role: "member" }], current: "vk" };
  await camps.start();
  voice.applyCampState({
    name: "Вечерний кооп",
    channels: opened("vk", "Вечерний кооп").channels,
    members: [{ userId: "bob", name: "Боб", role: "member", silenced: false }],
    presence: [],
  });
  await camps.setSilenced("bob", true);
  await camps.kick("bob");
  assert.deepEqual(api.calls.slice(-2), ["silence:vk:bob:true", "kick:vk:bob"]);
  assert.match(camps.store.get().notice ?? "", /^«Боб» выгнан с сервера/);

  api.failModeration = "Это может только тот, у кого роль выше";
  await camps.setSilenced("bob", false);
  assert.equal(camps.store.get().error, "Это может только тот, у кого роль выше");
});

test("your avatar comes from the device and goes back to it", async () => {
  const { api, camps } = setup();
  api.profile = Uint8Array.of(9, 9);
  await camps.start();
  await tick();
  assert.deepEqual(camps.store.get().avatar, Uint8Array.of(9, 9), "offered in the join form");
  await camps.setAvatar(Uint8Array.of(7));
  await camps.setAvatar(null);
  assert.deepEqual(api.calls.slice(-2), ["avatar:1", "avatar:none"]);
  assert.equal(camps.store.get().avatar, null);
});

test("each avatar is fetched once per hash; a failure is tried again", async () => {
  const { api, camps } = setup();
  api.list_ = { displayName: "Дима", camps: [{ id: "vk", name: "Вечерний кооп", role: "member" }], current: "vk" };
  await camps.start();
  await Promise.all([camps.avatarOf("bob", "h1"), camps.avatarOf("bob", "h1")]);
  assert.equal(api.avatarFetches, 1);
  await assert.rejects(camps.avatarOf("bob", "bad"));
  await tick();
  await assert.rejects(camps.avatarOf("bob", "bad"));
  assert.equal(api.avatarFetches, 3, "the failed one asked again");
});

test("an open camp's address is checked and joined like an invite", async () => {
  const { api, camps } = setup();
  await camps.start();
  camps.openJoin();
  const pending = camps.setLink(OPEN);
  assert.equal(camps.store.get().check.state, "checking", "the address alone is a link");
  api.pending.get(OPEN)?.resolve({ name: "OurCamp KZ", channels: 2, kind: "open", usable: true });
  await pending;
  camps.setName("Лена");
  assert.equal(camps.canJoin(), true);

  const closed = camps.setLink(`ourcamp://ourcamp.kz/?fp=${FP}`);
  api.pending.get(`ourcamp://ourcamp.kz/?fp=${FP}`)?.resolve({ name: "OurCamp KZ", channels: 2, kind: "open", usable: false });
  await closed;
  assert.equal(camps.canJoin(), false, "a camp that takes invites only");
});

test("the Чат button counts what is unread in the open camp's chats", async () => {
  const { api, camps } = setup();
  api.list_ = { displayName: "Дима", camps: [{ id: "vk", name: "Вечерний кооп", role: "member" }], current: "vk" };
  const withChats = (read: Record<string, number>): OpenCamp => ({
    ...opened("vk", "Вечерний кооп"),
    channels: [...opened("vk", "Вечерний кооп").channels, { id: "t1", name: "Общий", kind: "text", afk: false }],
    read,
  });
  api.open = (id) => {
    api.calls.push(`open:${id}`);
    return Promise.resolve(withChats({ t1: 3 }));
  };
  await camps.start();
  assert.equal(camps.store.get().unread, 0, "nothing known before the first snapshot");

  const state = (chats: Record<string, number>): CampLive => ({
    campId: "vk",
    online: true,
    state: { name: "Вечерний кооп", channels: withChats({}).channels, members: [], presence: [], chats },
  });
  api.emitLive(state({ t1: 5 }));
  assert.equal(camps.store.get().unread, 2);

  const message = (seq: number, authorId: string): ChatUpdate => ({
    type: "message",
    campId: "vk",
    message: { channelId: "t1", seq, authorId, authorName: "Боб", sentAt: 0, text: "Привет" },
  });
  api.emitChat(message(6, "bob"));
  assert.equal(camps.store.get().unread, 3, "a live message");
  api.emitLive(state({ t1: 5 }));
  assert.equal(camps.store.get().unread, 3, "a snapshot from before it does not take it back");
  api.emitChat({ ...message(9, "bob"), campId: "elsewhere" });
  assert.equal(camps.store.get().unread, 3, "another camp's chat");

  api.emitChat({ type: "read", campId: "vk", channelId: "t1", seq: 6 });
  assert.equal(camps.store.get().unread, 0, "read in the chat window");

  api.emitChat(message(7, "me"));
  assert.equal(camps.store.get().unread, 0, "your own message is read");
  assert.ok(api.calls.includes("read:vk:t1:7"), "and marked read on the device");
});
