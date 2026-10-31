import assert from "node:assert/strict";
import { test } from "node:test";
import {
  DEFAULT_STREAM_VOLUME,
  physicalRect,
  qualityText,
  readStreamVolume,
  saveStreamVolume,
  viewersText,
  waitingText,
  type ViewStatus,
} from "./stream-view.ts";

const live: ViewStatus = { live: true, height: 1080, fps: 60, viewers: 3, sound: true };
const off: ViewStatus = { live: false, height: 0, fps: 0, viewers: 1, sound: false };

test("the volume of streams is remembered, and a bad memory gives the default", () => {
  const stored = new Map<string, string>();
  const storage = { getItem: (k: string) => stored.get(k) ?? null, setItem: (k: string, v: string) => void stored.set(k, v) };
  assert.equal(readStreamVolume(storage), DEFAULT_STREAM_VOLUME);
  saveStreamVolume(storage, 35);
  assert.equal(readStreamVolume(storage), 35);
  saveStreamVolume(storage, 0);
  assert.equal(readStreamVolume(storage), 0, "silence is a real choice");
  stored.set("ourcamp.streamVolume", "громко");
  assert.equal(readStreamVolume(storage), DEFAULT_STREAM_VOLUME);
  stored.set("ourcamp.streamVolume", "900");
  assert.equal(readStreamVolume(storage), DEFAULT_STREAM_VOLUME);
  const broken = {
    getItem: () => {
      throw new Error("denied");
    },
    setItem: () => {
      throw new Error("denied");
    },
  };
  assert.equal(readStreamVolume(broken), DEFAULT_STREAM_VOLUME);
  saveStreamVolume(broken, 10);
});

test("the quality shows only while there is a picture", () => {
  assert.equal(qualityText(live), "1080p · 60 fps");
  assert.equal(qualityText(off), "");
});

test("the verb agrees with the number of viewers", () => {
  assert.equal(viewersText(1), "1 смотрит");
  assert.equal(viewersText(2), "2 смотрят");
  assert.equal(viewersText(5), "5 смотрят");
  assert.equal(viewersText(11), "11 смотрят");
  assert.equal(viewersText(21), "21 смотрит");
  assert.equal(viewersText(111), "111 смотрят");
});

test("without a picture the page says why", () => {
  assert.equal(waitingText(off, false), "Подключаемся к стриму…");
  assert.equal(waitingText(off, true), "Стрим закончился");
  assert.equal(waitingText(live, true), "");
});

test("the rectangle comes out in the window's pixels, without gaps at a fractional scale", () => {
  assert.deepEqual(physicalRect({ left: 0, top: 36, width: 960, height: 520 }, 1), { x: 0, y: 36, width: 960, height: 520 });
  assert.deepEqual(physicalRect({ left: 0, top: 36, width: 960, height: 520 }, 1.25), { x: 0, y: 45, width: 1200, height: 650 });
  // 36.5 × 1.5 = 54.75 → 55; the bottom edge 556.5 × 1.5 = 834.75 → 835.
  assert.deepEqual(physicalRect({ left: 0, top: 36.5, width: 100, height: 520 }, 1.5), { x: 0, y: 55, width: 150, height: 780 });
});
