import { test } from "node:test";
import assert from "node:assert/strict";
import { fitWithin, photoType, sizeLabel } from "./photo.ts";

test("photos shrink to 1600 px on their long side, never grow", () => {
  assert.deepEqual(fitWithin(4032, 3024), { width: 1600, height: 1200 });
  assert.deepEqual(fitWithin(1080, 2400), { width: 720, height: 1600 });
  assert.deepEqual(fitWithin(800, 600), { width: 800, height: 600 });
  assert.deepEqual(fitWithin(20000, 1), { width: 1600, height: 1 }, "a sliver keeps a pixel");
});

test("the type comes from the bytes, as the camp sees it", () => {
  assert.equal(photoType(Uint8Array.of(0xff, 0xd8, 0xff, 0xe0)), "image/jpeg");
  assert.equal(photoType(Uint8Array.of(0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a)), "image/png");
  const webp = new TextEncoder().encode("RIFF\0\0\0\0WEBPVP8 ");
  assert.equal(photoType(webp), "image/webp");
  assert.equal(photoType(new TextEncoder().encode("GIF89a")), null);
});

test("sizes read like a person says them", () => {
  assert.equal(sizeLabel(380 * 1024), "380 КБ");
  assert.equal(sizeLabel(1.4 * 1024 * 1024), "1,4 МБ");
  assert.equal(sizeLabel(10), "1 КБ");
});
