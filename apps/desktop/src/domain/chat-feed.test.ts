import { test } from "node:test";
import assert from "node:assert/strict";
import type { ChatMessage } from "../engine/camp-api.ts";
import { dayLabel, gapBefore, layoutFeed, mergeMessages, timeLabel } from "./chat-feed.ts";

const at = (y: number, m: number, d: number, h: number, min = 0) => new Date(y, m, d, h, min).getTime();
const NOW = at(2026, 9, 29, 21, 30);

const msg = (seq: number, authorId: string, sentAt: number, text = `№${seq}`): ChatMessage => ({
  channelId: "c1",
  seq,
  authorId,
  authorName: authorId,
  sentAt,
  text,
});

test("messages merge by number, each once, newest copy winning", () => {
  const held = [msg(3, "a", 3), msg(4, "a", 4)];
  const merged = mergeMessages(held, [msg(4, "a", 4, "правка"), msg(1, "b", 1), msg(5, "b", 5)]);
  assert.deepEqual(
    merged.map((m) => [m.seq, m.text]),
    [
      [1, "№1"],
      [3, "№3"],
      [4, "правка"],
      [5, "№5"],
    ],
  );
  assert.equal(gapBefore(held, 5), 0, "5 follows 4");
  assert.equal(gapBefore(held, 8), 3, "5, 6, 7 were missed");
  assert.equal(gapBefore([], 2), 1);
});

test("days read like people say them", () => {
  assert.equal(dayLabel(at(2026, 9, 29, 0, 5), NOW), "Сегодня");
  assert.equal(dayLabel(at(2026, 9, 28, 23, 59), NOW), "Вчера");
  assert.equal(dayLabel(at(2026, 9, 12, 12), NOW), "12 октября");
  assert.equal(dayLabel(at(2025, 2, 3, 12), NOW), "3 марта 2025");
  assert.equal(dayLabel(at(2026, 0, 31, 12), at(2026, 1, 1, 9)), "Вчера", "across a month");
  assert.equal(timeLabel(at(2026, 9, 29, 9, 5)), "09:05");
});

test("one author talking on is one turn; a pause, a new day or someone else starts another", () => {
  const feed = layoutFeed(
    [
      msg(1, "lena", at(2026, 9, 28, 22, 40)),
      msg(2, "lena", at(2026, 9, 28, 22, 41)),
      msg(3, "lena", at(2026, 9, 28, 22, 50)),
      msg(4, "max", at(2026, 9, 28, 22, 51)),
      msg(5, "max", at(2026, 9, 29, 9, 0)),
    ],
    NOW,
    "me",
    null,
  );
  assert.deepEqual(
    feed.map((i) => (i.kind === "message" ? `${i.message.seq}${i.head ? "+" : ""}` : i.kind === "day" ? i.label : "new")),
    ["Вчера", "1+", "2", "3+", "4+", "Сегодня", "5+"],
  );
});

test("a removed message leaves no trace in the feed, and its neighbours read on", () => {
  const removed: ChatMessage = { ...msg(2, "max", at(2026, 9, 29, 21, 1)), text: "", deleted: true };
  const feed = layoutFeed([msg(1, "lena", at(2026, 9, 29, 21, 0)), removed, msg(3, "lena", at(2026, 9, 29, 21, 2))], NOW, "me", 1);
  assert.deepEqual(
    feed.map((i) => (i.kind === "message" ? `${i.message.seq}${i.head ? "+" : ""}` : i.kind)),
    ["day", "1+", "unread", "3+"],
  );
  assert.deepEqual(layoutFeed([removed], NOW, "me", null), [], "nothing at all");
});

test("the new line goes before the first unread message of someone else", () => {
  const messages = [
    msg(1, "lena", at(2026, 9, 29, 21, 0)),
    msg(2, "me", at(2026, 9, 29, 21, 1)),
    msg(3, "lena", at(2026, 9, 29, 21, 2)),
  ];
  const kinds = (unreadAfter: number | null) =>
    layoutFeed(messages, NOW, "me", unreadAfter).map((i) => (i.kind === "message" ? i.message.seq : i.kind));
  assert.deepEqual(kinds(1), ["day", 1, 2, "unread", 3], "your own message is not news to you");
  assert.deepEqual(kinds(3), ["day", 1, 2, 3], "all read");
  assert.deepEqual(kinds(null), ["day", 1, 2, 3]);
  const feed = layoutFeed(messages, NOW, "me", 1);
  const third = feed.at(-1);
  assert.ok(third?.kind === "message" && third.head, "after the line the author shows again");
});
