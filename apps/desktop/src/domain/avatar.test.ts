import { test } from "node:test";
import assert from "node:assert/strict";
import { avatarFormat, cropSquare, decodeBase64Url, encodeBase64Url } from "./avatar.ts";

test("a round avatar shows the centred square", () => {
  assert.deepEqual(cropSquare(400, 300), { sx: 50, sy: 0, side: 300 }, "landscape");
  assert.deepEqual(cropSquare(300, 401), { sx: 0, sy: 50, side: 300 }, "portrait");
  assert.deepEqual(cropSquare(128, 128), { sx: 0, sy: 0, side: 128 });
});

test("the format is the camp's call: WebP or PNG by signature", () => {
  const bytes = (s: string) => Uint8Array.from(s, (c) => c.charCodeAt(0));
  assert.equal(avatarFormat(bytes("RIFF\x10\0\0\0WEBPVP8 ")), "image/webp");
  assert.equal(avatarFormat(bytes("\x89PNG\r\n\x1a\n\0")), "image/png");
  assert.equal(avatarFormat(bytes("GIF89a")), null);
  assert.equal(avatarFormat(bytes("<svg/>")), null);
});

test("base64url round-trips any bytes, as the Rust side writes them", () => {
  const all = Uint8Array.from({ length: 256 }, (_, i) => i);
  const text = encodeBase64Url(all);
  assert.match(text, /^[A-Za-z0-9_-]+$/, "no padding, no + or /");
  assert.deepEqual(decodeBase64Url(text), all);
  assert.equal(encodeBase64Url(Uint8Array.of(0xfb, 0xff)), "-_8");
  const big = new Uint8Array(200_000).map((_, i) => i % 251);
  assert.deepEqual(decodeBase64Url(encodeBase64Url(big)), big, "large pictures too");
});
