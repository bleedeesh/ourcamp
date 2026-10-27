import { test } from "node:test";
import assert from "node:assert/strict";
import type { Camp } from "./types.ts";
import { applyRoster, avatarColor } from "./roster.ts";

const member = (id: string, extra = {}) => ({
  id,
  name: id.toUpperCase(),
  color: "#888",
  micMuted: false,
  deafened: false,
  isAdmin: false,
  streaming: false,
  ...extra,
});

const camp: Camp = {
  id: "vk",
  name: "Вечерний кооп",
  short: "ВК",
  color: "#8E44AD",
  channels: [
    { id: "lobby", name: "Лобби", memberIds: ["artem", "lena"] },
    { id: "katka", name: "Катка", memberIds: ["olya"] },
  ],
  members: { me: member("me"), artem: member("artem", { isAdmin: true }), lena: member("lena"), olya: member("olya") },
};

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
  assert.equal(next.members["artem"]?.isAdmin, true);
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
