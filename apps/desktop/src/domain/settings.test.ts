import assert from "node:assert/strict";
import { test } from "node:test";
import { canSaveName, MAX_NAME_CHARS, nameProblem } from "./settings.ts";

test("a name follows the camp's rule", () => {
  assert.equal(nameProblem("Дима"), null);
  assert.equal(nameProblem("  Дима  "), null);
  assert.equal(nameProblem("я".repeat(MAX_NAME_CHARS)), null);
  assert.equal(nameProblem("   "), "Введите имя");
  assert.match(nameProblem("я".repeat(MAX_NAME_CHARS + 1)) ?? "", /Не длиннее 32/);
  assert.match(nameProblem("а\nб") ?? "", /управляющих/);
});

test("characters are counted, not UTF-16 units", () => {
  assert.equal(nameProblem("🔥".repeat(MAX_NAME_CHARS)), null);
});

test("only a changed, fitting name can be saved", () => {
  assert.equal(canSaveName("Дима", "Дима"), false);
  assert.equal(canSaveName(" Дима ", "Дима"), false, "spaces around do not count");
  assert.equal(canSaveName("Дмитрий", "Дима"), true);
  assert.equal(canSaveName("   ", "Дима"), false);
});
