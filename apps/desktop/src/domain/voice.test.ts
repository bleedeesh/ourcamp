import { test } from "node:test";
import assert from "node:assert/strict";
import { FakeEngine, MemoryStorage } from "./test-support.ts";
import type { Camp } from "./types.ts";
import { VoiceController } from "./voice.ts";
import { VolumeBook } from "./volume.ts";

const member = (id: string) => ({
  id,
  name: id,
  color: "#888",
  micMuted: false,
  deafened: false,
  role: "member" as const,
  streaming: false,
});

const camp: Camp = {
  id: "vk",
  name: "Вечерний кооп",
  short: "ВК",
  color: "#8E44AD",
  channels: [
    { id: "lobby", name: "Лобби", afk: false, memberIds: ["artem", "lena"] },
    { id: "afk", name: "AFK", afk: true, memberIds: [] },
  ],
  members: { artem: member("artem"), lena: member("lena"), me: member("me") },
};

function setup() {
  const engine = new FakeEngine();
  const storage = new MemoryStorage();
  const volumes = new VolumeBook(storage);
  const voice = new VoiceController(engine, volumes, camp, "me");
  voice.start();
  return { engine, volumes, voice, storage };
}

test("joining passes saved volumes of present members to the engine", async () => {
  const { engine, volumes, voice } = setup();
  volumes.setVolume("lena", 150);
  await voice.join("lobby");
  assert.equal(voice.store.get().channelId, "lobby");
  assert.deepEqual(engine.joined?.members, [
    { userId: "artem", volume: 100, muted: false, micMuted: false },
    { userId: "lena", volume: 150, muted: false, micMuted: false },
  ]);
});

test("failed join rolls back and reports the error", async () => {
  const { engine, voice } = setup();
  engine.failNext = true;
  await voice.join("lobby");
  assert.equal(voice.store.get().channelId, null);
  assert.equal(voice.store.get().error, "device lost");
});

test("rejoining the same channel is a no-op; leave clears speakers", async () => {
  const { engine, voice } = setup();
  await voice.join("lobby");
  await voice.join("lobby");
  engine.emit([{ userId: "lena", levelDb: -20, speaking: true }]);
  await voice.leave();
  assert.deepEqual(engine.calls, ["join:lobby", "leave"]);
  assert.deepEqual(voice.store.get().speaking, {});
});

test("deafen mutes the mic and undeafen restores the previous mic state", async () => {
  const { engine, voice } = setup();
  await voice.toggleDeafen();
  assert.equal(voice.store.get().micMuted, true);
  await voice.toggleDeafen();
  assert.equal(voice.store.get().micMuted, false, "mic was on before deafening");

  await voice.toggleMic(); // mute
  await voice.toggleDeafen();
  await voice.toggleDeafen();
  assert.equal(voice.store.get().micMuted, true, "mic was off before deafening");
  assert.ok(engine.calls.includes("deaf:true"));
});

test("unmuting while deafened undeafens too", async () => {
  const { voice } = setup();
  await voice.toggleDeafen();
  await voice.toggleMic();
  assert.equal(voice.store.get().deafened, false);
  assert.equal(voice.store.get().micMuted, false);
});

test("volume changes are clamped, persisted and sent to the engine", async () => {
  const { engine, voice, storage } = setup();
  await voice.setVolume("lena", 250);
  assert.deepEqual(voice.prefsOf("lena"), { volume: 200, muted: false });
  assert.ok(engine.calls.includes("volume:lena:200"));
  assert.match(storage.getItem("ourcamp.volumes.v1") ?? "", /"lena"/);
});

test("local mute toggles and reset restores defaults", async () => {
  const { engine, voice } = setup();
  await voice.setVolume("artem", 60);
  await voice.toggleLocalMute("artem");
  assert.deepEqual(voice.prefsOf("artem"), { volume: 60, muted: true });
  await voice.resetVolume("artem");
  assert.deepEqual(voice.prefsOf("artem"), { volume: 100, muted: false });
  assert.deepEqual(engine.calls.slice(-2), ["volume:artem:100", "localMute:artem:false"]);
});

test("speaking updates only touch the store when the speaker set changes", () => {
  const { engine, voice } = setup();
  let renders = 0;
  voice.store.subscribe(() => renders++);
  engine.emit([{ userId: "lena", levelDb: -20, speaking: true }]);
  engine.emit([{ userId: "lena", levelDb: -18, speaking: true }]);
  engine.emit([{ userId: "lena", levelDb: -60, speaking: false }]);
  assert.equal(renders, 2);
  assert.deepEqual(voice.store.get().speaking, {});
});

test("the room's member list replaces the channel's and adopts the room's self id", async () => {
  const { engine, voice } = setup();
  await voice.join("lobby");
  engine.announce({
    selfId: "me",
    members: [
      { userId: "lena", name: "lena", micMuted: true },
      { userId: "zhenya", name: "Женя", micMuted: false },
    ],
  });
  const { camp, selfId } = voice.store.get();
  assert.deepEqual(camp.channels[0]?.memberIds, ["lena", "zhenya"]);
  assert.equal(camp.members["lena"]?.micMuted, true);
  assert.equal(camp.members["zhenya"]?.name, "Женя");
  assert.equal(selfId, "me");

  engine.announce({ selfId: "artem", members: [] });
  assert.equal(voice.store.get().selfId, "artem", "identity comes from the room");
});

test("people who join later get their saved volume; known ones are not resent", async () => {
  const { engine, volumes, voice } = setup();
  volumes.setVolume("zhenya", 40);
  volumes.setMuted("zhenya", true);
  await voice.join("lobby");
  engine.calls = [];
  const update = {
    selfId: "me",
    members: [
      { userId: "artem", name: "artem", micMuted: false },
      { userId: "zhenya", name: "Женя", micMuted: false },
    ],
  };
  engine.announce(update);
  engine.announce(update);
  await Promise.resolve();
  await Promise.resolve();
  assert.deepEqual(engine.calls, ["volume:zhenya:40", "localMute:zhenya:true"]);
  assert.deepEqual(voice.prefsOf("zhenya"), { volume: 40, muted: true });
});

test("membership updates outside a channel are ignored", async () => {
  const { engine, voice } = setup();
  await voice.join("lobby");
  engine.announce({ selfId: "me", members: [{ userId: "lena", name: "lena", micMuted: false }] });
  await voice.leave();
  engine.announce({ selfId: "me", members: [] });
  assert.deepEqual(voice.store.get().camp.channels[0]?.memberIds, ["lena"], "last known list kept");
});

test("volume menu cannot be opened for yourself", () => {
  const { voice } = setup();
  voice.openVolumeMenu({ userId: "me", x: 0, y: 0 });
  assert.equal(voice.store.get().volumeMenu, null);
  voice.openVolumeMenu({ userId: "lena", x: 10, y: 20 });
  assert.equal(voice.store.get().volumeMenu?.userId, "lena");
  voice.closeVolumeMenu();
  assert.equal(voice.store.get().volumeMenu, null);
});

test("a server-side disconnect leaves the campfire and says why", async () => {
  const { engine, voice } = setup();
  await voice.join("lobby");
  engine.emit([{ userId: "lena", levelDb: -20, speaking: true }]);
  engine.disconnect("Вы зашли в этот лагерь из другого окна");
  const s = voice.store.get();
  assert.equal(s.channelId, null);
  assert.deepEqual(s.speaking, {});
  assert.equal(s.error, "Вы зашли в этот лагерь из другого окна");

  // A stale event after we already left changes nothing.
  voice.dismissError();
  engine.disconnect("late");
  assert.equal(voice.store.get().error, null);
});

test("the AFK campfire turns mic and sound off before connecting and restores them after", async () => {
  const { engine, voice } = setup();
  await voice.join("lobby");
  await voice.toggleMic(); // muted on purpose before going AFK
  engine.calls = [];

  await voice.join("afk");
  assert.deepEqual(
    engine.calls.slice(0, 3),
    ["deaf:true", "mic:true", "join:afk"],
    "silenced before the room connects",
  );
  assert.equal(voice.store.get().deafened, true);
  assert.equal(voice.store.get().micMuted, true);

  await voice.toggleDeafen(); // listening in AFK is allowed; the mic stays off
  assert.equal(voice.store.get().deafened, false);
  assert.equal(voice.store.get().micMuted, true);

  await voice.join("lobby");
  assert.equal(voice.store.get().deafened, false, "as before AFK");
  assert.equal(voice.store.get().micMuted, true, "it was muted before AFK, so it stays muted");
});

test("leaving AFK any way gives the sound back", async () => {
  const { voice } = setup();
  await voice.join("afk");
  assert.equal(voice.store.get().deafened, true);
  await voice.leave();
  assert.equal(voice.store.get().deafened, false);
  assert.equal(voice.store.get().micMuted, false);
});
