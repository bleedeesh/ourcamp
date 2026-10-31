import assert from "node:assert/strict";
import { test } from "node:test";
import { physicalRect, qualityText, viewersText, waitingText, type ViewStatus } from "./stream-view.ts";

const live: ViewStatus = { live: true, height: 1080, fps: 60, viewers: 3 };
const off: ViewStatus = { live: false, height: 0, fps: 0, viewers: 1 };

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
