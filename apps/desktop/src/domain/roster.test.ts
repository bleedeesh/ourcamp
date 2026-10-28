import { test } from "node:test";
import assert from "node:assert/strict";
import type { CampState } from "../engine/camp-api.ts";
import type { Camp } from "./types.ts";
import { applyCampState, applyRoster, avatarColor, canGovern, canMove, governs, moves } from "./roster.ts";

const member = (id: string, extra = {}) => ({
  id,
  name: id.toUpperCase(),
  color: "#888",
  micMuted: false,
  deafened: false,
  silenced: false,
  role: "member" as const,
  streaming: false,
  ...extra,
});

const camp: Camp = {
  id: "vk",
  name: "Вечерний кооп",
  short: "ВК",
  color: "#8E44AD",
  channels: [
    { id: "lobby", name: "Лобби", afk: false, memberIds: ["artem", "lena"] },
    { id: "katka", name: "Катка", afk: false, memberIds: ["olya"] },
  ],
  members: { me: member("me"), artem: member("artem", { role: "admin" as const }), lena: member("lena"), olya: member("olya") },
};

test("moderators and admins move their rank and below; anyone moves themselves", () => {
  assert.equal(moves("member", "member"), false);
  assert.equal(moves("moderator", "moderator"), true);
  assert.equal(moves("moderator", "admin"), false);
  assert.equal(moves("admin", "admin"), true);
  assert.equal(canMove(camp, "me", "me"), true, "yourself: just a join");
  assert.equal(canMove(camp, "me", "lena"), false, "a member moves nobody else");
  assert.equal(canMove(camp, "artem", "lena"), true, "the admin does");
  assert.equal(canMove(camp, "artem", "ghost"), false, "unknown people stay put");
});

test("kicking and silencing reach lower ranks only, never yourself", () => {
  assert.equal(governs("member", "member"), false);
  assert.equal(governs("moderator", "member"), true);
  assert.equal(governs("moderator", "moderator"), false, "moderators do not kick each other");
  assert.equal(governs("admin", "moderator"), true);
  assert.equal(governs("admin", "admin"), false, "only the owner kicks an admin");
  assert.equal(canGovern(camp, "artem", "lena"), true, "the admin over a member");
  assert.equal(canGovern(camp, "artem", "artem"), false, "not yourself");
  assert.equal(canGovern(camp, "me", "lena"), false);
});

test("the joined channel holds exactly the room's members", () => {
  const next = applyRoster(camp, "lobby", {
    selfId: "me",
    members: [{ userId: "artem", name: "artem", micMuted: false }],
  });
  assert.deepEqual(next.channels[0]?.memberIds, ["artem"]);
  assert.deepEqual(next.channels[1]?.memberIds, ["olya"], "other channels untouched");
});

test("a person moves out of the channel they were listed in", () => {
  const next = applyRoster(camp, "lobby", {
    selfId: "me",
    members: [{ userId: "olya", name: "olya", micMuted: false }],
  });
  assert.deepEqual(next.channels[1]?.memberIds, []);
});

test("known members keep their profile; mic state follows the room", () => {
  const next = applyRoster(camp, "lobby", {
    selfId: "me",
    members: [{ userId: "artem", name: "someone else", micMuted: true }],
  });
  assert.equal(next.members["artem"]?.name, "ARTEM");
  assert.equal(next.members["artem"]?.role, "admin");
  assert.equal(next.members["artem"]?.micMuted, true);
});

test("strangers get a profile from the room and a stable colour", () => {
  const next = applyRoster(camp, "lobby", {
    selfId: "me",
    members: [
      { userId: "zhenya", name: "Женя", micMuted: false },
      { userId: "anon", name: "  ", micMuted: true },
    ],
  });
  assert.equal(next.members["zhenya"]?.name, "Женя");
  assert.equal(next.members["zhenya"]?.color, avatarColor("zhenya"));
  assert.equal(next.members["anon"]?.name, "anon", "blank names fall back to the id");
  assert.equal(next.members["anon"]?.micMuted, true);
});

test("self id from the room gets a profile and is never listed as a remote member", () => {
  const next = applyRoster(camp, "lobby", { selfId: "lena", members: [] });
  assert.ok(next.members["lena"]);
  assert.deepEqual(next.channels[0]?.memberIds, []);
  const stranger = applyRoster(camp, "lobby", { selfId: "guest", members: [] });
  assert.equal(stranger.members["guest"]?.name, "guest");
});

test("avatar colour is deterministic", () => {
  assert.equal(avatarColor("artem"), avatarColor("artem"));
  assert.match(avatarColor(""), /^#[0-9A-F]{6}$/i);
});

const state = (presence: Record<string, Array<[string, boolean]>>): CampState => ({
  name: "Вечерний кооп",
  channels: [
    { id: "lobby", name: "Лобби", kind: "voice", afk: false },
    { id: "katka", name: "Катка", kind: "voice", afk: false },
  ],
  members: [
    { userId: "me", name: "ME", role: "member", silenced: false },
    { userId: "artem", name: "Артём", role: "admin", silenced: false },
    { userId: "zhenya", name: "Женя", role: "moderator", silenced: true },
  ],
  presence: Object.entries(presence).map(([channelId, people]) => ({
    channelId,
    members: people.map(([userId, micMuted]) => ({ userId, micMuted })),
  })),
});

test("presence fills every campfire, with names and roles from the camp", () => {
  const next = applyCampState(camp, state({ katka: [["zhenya", true]], lobby: [["artem", false]] }), null, "me");
  assert.deepEqual(next.channels[0]?.memberIds, ["artem"]);
  assert.deepEqual(next.channels[1]?.memberIds, ["zhenya"]);
  assert.equal(next.members["zhenya"]?.name, "Женя");
  assert.equal(next.members["zhenya"]?.micMuted, true);
  assert.equal(next.members["artem"]?.role, "admin", "the crown comes from the server");
  assert.equal(next.members["zhenya"]?.role, "moderator");
  assert.equal(next.members["zhenya"]?.silenced, true, "forbidden to speak, from the server");
  assert.equal(next.members["artem"]?.silenced, false);
});

test("empty campfires empty out; you are never listed by presence", () => {
  const next = applyCampState(camp, state({ lobby: [["me", false]] }), null, "me");
  assert.deepEqual(next.channels[0]?.memberIds, []);
  assert.deepEqual(next.channels[1]?.memberIds, []);
});

test("your own campfire keeps the room's view; nobody shows up twice", () => {
  const inLobby = { ...camp, channels: camp.channels.map((c) => (c.id === "lobby" ? { ...c, memberIds: ["lena"] } : c)) };
  // A stale poll still places Lena at Katka and Artem at the Lobby.
  const next = applyCampState(inLobby, state({ lobby: [["artem", false]], katka: [["lena", false]] }), "lobby", "me");
  assert.deepEqual(next.channels[0]?.memberIds, ["lena"], "room roster wins for your campfire");
  assert.deepEqual(next.channels[1]?.memberIds, [], "Lena is with you, not at Katka");
});

test("renames and campfire changes arrive live; chats stay out of the tree", () => {
  const live: CampState = {
    ...state({ katka: [["zhenya", false]] }),
    name: "Новый сервер",
    channels: [
      { id: "katka", name: "Катка 2", kind: "voice", afk: false },
      { id: "chat", name: "Chat", kind: "text", afk: false },
      { id: "fresh", name: "Новый канал", kind: "voice", afk: false },
    ],
  };
  const inLobby = { ...camp, channels: camp.channels.map((c) => (c.id === "lobby" ? { ...c, memberIds: ["lena"] } : c)) };
  const next = applyCampState(inLobby, live, "lobby", "me");
  assert.equal(next.name, "Новый сервер");
  assert.equal(next.short, "НС", "the badge follows the name");
  assert.deepEqual(
    next.channels.map((c) => [c.id, c.name, c.memberIds]),
    [
      ["katka", "Катка 2", ["zhenya"]],
      ["fresh", "Новый канал", []],
    ],
    "the removed Lobby is gone, the chat is not a campfire, order follows the camp",
  );
});
