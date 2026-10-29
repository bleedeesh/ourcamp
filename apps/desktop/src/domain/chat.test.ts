import { test } from "node:test";
import assert from "node:assert/strict";
import type { ChannelInfo } from "../engine/camp-api.ts";
import { advance, badgeText, unreadCount, unreadLabel } from "./chat.ts";

const channels: ChannelInfo[] = [
  { id: "c0", name: "Voice", kind: "voice", afk: false },
  { id: "c1", name: "Chat", kind: "text", afk: false },
  { id: "c2", name: "Флуд", kind: "text", afk: false },
];

test("unread is each chat's last message minus the last one read", () => {
  assert.equal(unreadCount(channels, { c1: 5, c2: 2 }, { c1: 3 }), 4, "2 in Chat, 2 in a chat never opened");
  assert.equal(unreadCount(channels, { c1: 5 }, { c1: 5 }), 0);
  assert.equal(unreadCount(channels, { c1: 5 }, { c1: 9 }), 0, "read ahead of a stale snapshot is not negative");
  assert.equal(unreadCount(channels, { c0: 4, gone: 7 }, {}), 0, "voice channels and removed chats do not count");
});

test("read marks and latest numbers only move forward", () => {
  const seqs = { c1: 3 };
  assert.deepEqual(advance(seqs, "c1", 5), { c1: 5 });
  assert.equal(advance(seqs, "c1", 2), seqs, "unchanged, the same object");
  assert.deepEqual(advance(seqs, "c2", 1), { c1: 3, c2: 1 });
});

test("the badge and its label read naturally", () => {
  assert.equal(badgeText(0), "");
  assert.equal(badgeText(7), "7");
  assert.equal(badgeText(120), "99+");
  assert.equal(unreadLabel(1), "1 непрочитанное");
  assert.equal(unreadLabel(11), "11 непрочитанных");
  assert.equal(unreadLabel(21), "21 непрочитанное");
  assert.equal(unreadLabel(3), "3 непрочитанных");
});
