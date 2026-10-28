import { test } from "node:test";
import assert from "node:assert/strict";
import type { CampApi, CampList, CampPreview, OpenCamp } from "../engine/camp-api.ts";
import { campfires, CampsController, campShort, NO_CAMP } from "./camps.ts";
import { FakeEngine, MemoryStorage } from "./test-support.ts";
import { VoiceController } from "./voice.ts";
import { VolumeBook } from "./volume.ts";

const FP = "A".repeat(43);
const LINK = `ourcamp://192.0.2.15/join/7k2q9m4x8v1c3b5n?fp=${FP}`;
const CLAIM = `ourcamp://192.0.2.15/claim/bcmqj75a9v64svgfthvb6fq4mw?fp=${FP}`;

const opened = (id: string, name: string, offline = false): OpenCamp => ({
  id,
  name,
  channels: [
    { id: "c0", name: "Лобби" },
    { id: "c1", name: "Катка" },
  ],
  self: { userId: "me", name: "Дима", isAdmin: id === "own" },
  offline,
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
  forget(): Promise<void> {
    return Promise.resolve();
  }
  invite(id: string): Promise<string> {
    this.calls.push(`invite:${id}`);
    return Promise.resolve(LINK);
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
  assert.equal(api.calls.length, 0);
  await camps.setLink("https://example.com");
  const check = camps.store.get().check;
  assert.equal(check.state, "error");
  assert.match(check.state === "error" ? check.message : "", /ourcamp:\/\//);
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
  assert.equal(campShort("лагерь"), "ЛА");
  assert.equal(campShort("  "), "?");
  assert.equal(campfires(1), "1 костёр");
  assert.equal(campfires(3), "3 костра");
  assert.equal(campfires(5), "5 костров");
  assert.equal(campfires(11), "11 костров");
  assert.equal(campfires(22), "22 костра");
});
