import { test } from "node:test";
import assert from "node:assert/strict";
import { fitWithin, gifRefusal, isMovingWebp, MAX_GIF_BYTES, photoType, sizeLabel } from "./photo.ts";

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
  assert.equal(photoType(new TextEncoder().encode("GIF87a")), "image/gif");
  assert.equal(photoType(new TextEncoder().encode("GIF89a")), "image/gif");
  assert.equal(photoType(new TextEncoder().encode("GIF88a")), null);
  assert.equal(photoType(new TextEncoder().encode("BM....")), null);
});

test("a moving WebP is told from a still one", () => {
  const webp = (chunk: string, flags: number) => {
    const head = new TextEncoder().encode(`RIFF\0\0\0\0WEBP${chunk}\0\0\0\0\0`);
    head[20] = flags;
    return head;
  };
  assert.equal(isMovingWebp(webp("VP8X", 0x02)), true);
  assert.equal(isMovingWebp(webp("VP8X", 0x10)), false, "extended, but still");
  assert.equal(isMovingWebp(webp("VP8 ", 0x02)), false, "a simple one has no such flag");
  assert.equal(isMovingWebp(new TextEncoder().encode("GIF89a")), false);
});

test("a GIF goes up to its limits and says which one it crossed", () => {
  assert.equal(gifRefusal(MAX_GIF_BYTES, 4096, 4096), null);
  assert.equal(gifRefusal(MAX_GIF_BYTES + 1, 480, 270), "GIF весит больше 8,0 МБ — такой отправить нельзя");
  assert.equal(gifRefusal(1024, 4097, 10), "GIF слишком большой: можно до 4096 точек по длинной стороне");
});

test("sizes read like a person says them", () => {
  assert.equal(sizeLabel(380 * 1024), "380 КБ");
  assert.equal(sizeLabel(1.4 * 1024 * 1024), "1,4 МБ");
  assert.equal(sizeLabel(10), "1 КБ");
});
