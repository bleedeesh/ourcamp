import assert from "node:assert/strict";
import { test } from "node:test";

import { findByPrefix, moveHighlight } from "./select.ts";

test("arrows move within the list and stop at its ends", () => {
  assert.equal(moveHighlight(-1, "ArrowDown", 3), 0, "first press: the first");
  assert.equal(moveHighlight(-1, "ArrowUp", 3), 2, "or the last");
  assert.equal(moveHighlight(1, "ArrowDown", 3), 2);
  assert.equal(moveHighlight(2, "ArrowDown", 3), 2, "no wrap past the end");
  assert.equal(moveHighlight(0, "ArrowUp", 3), 0);
  assert.equal(moveHighlight(1, "Home", 3), 0);
  assert.equal(moveHighlight(1, "End", 3), 2);
  assert.equal(moveHighlight(1, "Enter", 3), null, "not a move");
  assert.equal(moveHighlight(-1, "ArrowDown", 0), null, "nothing to move in");
});

test("typing finds an option by its first letters", () => {
  const options = [
    { value: "auto", label: "Автоматически" },
    { value: "lan", label: "Локальная сеть" },
    { value: "radmin", label: "Radmin VPN" },
    { value: "custom", label: "Свой адрес" },
    { value: "admin", label: "Админ" },
  ];
  assert.equal(findByPrefix(options, "р", -1), null, "Cyrillic р is no Latin R");
  assert.equal(findByPrefix(options, "r", -1), 2);
  assert.equal(findByPrefix(options, "с", 0), 3);
  assert.equal(findByPrefix(options, "а", 0), 4, "the same letter again: the next match");
  assert.equal(findByPrefix(options, "а", 4), 0, "and round");
  assert.equal(findByPrefix(options, "ав", 0), 0, "a longer prefix stays on a match");
  assert.equal(findByPrefix(options, "x", 0), null);
});
