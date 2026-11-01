import assert from "node:assert/strict";
import { test } from "node:test";

import {
  AS_IN_SYSTEM,
  deviceChoice,
  deviceOptions,
  deviceValue,
  hotkeyFromMouse,
  hotkeyFromPress,
  MAX_THRESHOLD_DB,
  meterPercent,
  MIN_THRESHOLD_DB,
  plainKey,
  sameKey,
  talkKeyFromMouse,
  talkKeyFromPress,
  thresholdLabel,
} from "./sound.ts";

test("a key is named by its place and kept by its virtual-key code", () => {
  assert.deepEqual(talkKeyFromPress("KeyV", 0x56), { code: 0x56, label: "V" });
  assert.deepEqual(talkKeyFromPress("Digit1", 0x31), { code: 0x31, label: "1" });
  assert.deepEqual(talkKeyFromPress("F13", 0x7c), { code: 0x7c, label: "F13" });
  assert.deepEqual(talkKeyFromPress("Space", 0x20), { code: 0x20, label: "Пробел" });
  assert.deepEqual(talkKeyFromPress("ControlRight", 0x11), { code: 0x11, label: "Ctrl" });
  assert.deepEqual(talkKeyFromPress("Numpad5", 0x65), { code: 0x65, label: "Num 5" });
});

test("what cannot be a talk key is refused", () => {
  assert.equal(talkKeyFromPress("MetaLeft", 0x5b), null, "opens the system's menu");
  assert.equal(talkKeyFromPress("", 0), null, "an unknown key has no code");
});

test("only the side buttons of a mouse can be a talk key", () => {
  assert.deepEqual(talkKeyFromMouse(3), { code: 5, label: "Кнопка мыши 4" });
  assert.deepEqual(talkKeyFromMouse(4), { code: 6, label: "Кнопка мыши 5" });
  for (const button of [0, 1, 2]) assert.equal(talkKeyFromMouse(button), null);
});

const NONE = { ctrl: false, shift: false, alt: false };
const CTRL = { ...NONE, ctrl: true };

test("a combination is named in the usual order", () => {
  assert.deepEqual(hotkeyFromPress("KeyM", 0x4d, { ctrl: true, shift: true, alt: false }), {
    code: 0x4d,
    ctrl: true,
    shift: true,
    alt: false,
    label: "Ctrl+Shift+M",
  });
});

test("a key that types needs a modifier, an F-key does not", () => {
  assert.equal(hotkeyFromPress("KeyM", 0x4d, NONE), null);
  assert.equal(hotkeyFromPress("Space", 0x20, NONE), null);
  assert.equal(hotkeyFromPress("F9", 0x78, NONE)?.label, "F9");
  assert.equal(hotkeyFromPress("KeyM", 0x4d, CTRL)?.label, "Ctrl+M");
});

test("a modifier alone is only the start of a combination", () => {
  assert.equal(hotkeyFromPress("ControlLeft", 0x11, CTRL), null);
  assert.equal(hotkeyFromPress("AltRight", 0x12, { ...NONE, alt: true }), null);
  assert.equal(hotkeyFromPress("MetaLeft", 0x5b, CTRL), null);
});

test("only a side mouse button can be a key, with modifiers too", () => {
  assert.equal(hotkeyFromMouse(0, CTRL), null);
  assert.equal(hotkeyFromMouse(3, NONE)?.label, "Кнопка мыши 4");
  assert.equal(hotkeyFromMouse(4, CTRL)?.label, "Ctrl+Кнопка мыши 5");
});

test("the same key means the same code and modifiers", () => {
  const ctrlM = hotkeyFromPress("KeyM", 0x4d, CTRL)!;
  assert.equal(sameKey(ctrlM, { ...ctrlM, label: "другая подпись" }), true);
  assert.equal(sameKey(ctrlM, { ...ctrlM, shift: true }), false);
  assert.equal(sameKey(ctrlM, null), false);
  assert.equal(sameKey(hotkeyFromPress("F9", 0x78, NONE)!, plainKey({ code: 0x78, label: "F9" })), true, "the talk key");
  assert.equal(sameKey(ctrlM, plainKey({ code: 0x4d, label: "M" })), false, "M alone is another key");
  assert.equal(plainKey(null), null);
});

test("the bar covers the thresholds on offer and no more", () => {
  assert.equal(meterPercent(MIN_THRESHOLD_DB), 0);
  assert.equal(meterPercent(MAX_THRESHOLD_DB), 100);
  assert.equal(meterPercent(-40), 50);
  assert.equal(meterPercent(-100), 0, "silence");
  assert.equal(meterPercent(0), 100, "louder than any threshold");
});

test("the threshold is said in whole decibels with a real minus", () => {
  assert.equal(thresholdLabel(-50), "−50 дБ");
  assert.equal(thresholdLabel(-33.4), "−33 дБ");
});

test("the system's device comes first, named, then every device", () => {
  const options = deviceOptions(["Наушники", "Динамики"], "Динамики", null);
  assert.deepEqual(options, [
    { value: AS_IN_SYSTEM, label: "Как в системе: Динамики" },
    { value: "Наушники", label: "Наушники" },
    { value: "Динамики", label: "Динамики" },
  ]);
});

test("a chosen device that is not connected stays on the list", () => {
  const options = deviceOptions(["Динамики"], "Динамики", "Наушники");
  assert.deepEqual(options.at(-1), { value: "Наушники", label: "Наушники", detail: "не подключено" });
});

test("a connected choice is listed once", () => {
  const options = deviceOptions(["Наушники"], "Наушники", "Наушники");
  assert.equal(options.filter((o) => o.value === "Наушники").length, 1);
});

test("with no device at all the list says so", () => {
  assert.deepEqual(deviceOptions([], null, null), [
    { value: AS_IN_SYSTEM, label: "Как в системе", detail: "нет устройства" },
  ]);
});

test("a choice and a list's value turn into each other", () => {
  assert.equal(deviceValue(null), AS_IN_SYSTEM);
  assert.equal(deviceChoice(AS_IN_SYSTEM), null);
  assert.equal(deviceChoice(deviceValue("Наушники")), "Наушники");
});
