import assert from "node:assert/strict";
import { test } from "node:test";

import { crowd, headline, nameProblem, plural, STARTING, voice, type Member } from "./status.ts";

const member = (name: string, campfire: string | null = null): Member => ({
  userId: name,
  name,
  role: "member",
  campfire,
  micMuted: false,
});

test("headline follows the phase", () => {
  assert.equal(headline(STARTING), "Сервер запускается…");
  assert.equal(headline({ ...STARTING, phase: { kind: "running" } }), "Сервер работает");
  assert.equal(headline({ ...STARTING, phase: { kind: "failed", message: "x" } }), "Сервер не запустился");
});

test("crowd counts people and who sits at a campfire", () => {
  assert.equal(crowd([]), "Пока никого — отправьте друзьям ссылку");
  assert.equal(crowd([member("Дима")]), "1 человек на сервере");
  assert.equal(crowd([member("Дима", "Лобби"), member("Лена")]), "2 человека на сервере · 1 в голосе");
});

test("voice shows a LiveKit restart", () => {
  const running = { ...STARTING, phase: { kind: "running" } as const };
  assert.equal(voice({ ...running, livekitPid: 42 }), "Голос: работает (LiveKit, pid 42)");
  assert.equal(voice(running), "Голос: перезапускается…");
  assert.equal(voice(STARTING), "Голос: —");
});

test("plural covers the teens", () => {
  const p = (n: number) => plural(n, "человек", "человека", "человек");
  assert.deepEqual([1, 2, 5, 11, 12, 21, 22, 25, 111].map(p), [
    "человек",
    "человека",
    "человек",
    "человек",
    "человек",
    "человек",
    "человека",
    "человек",
    "человек",
  ]);
});

test("names are checked like the camp checks them", () => {
  assert.equal(nameProblem("   ", 32), "Введите название");
  assert.equal(nameProblem("  Кооп  ", 32), null, "trimmed first");
  assert.equal(nameProblem("я".repeat(32), 32), null, "characters, not bytes");
  assert.equal(nameProblem("я".repeat(33), 32), "Не длиннее 32 символов");
});
