import { test } from "node:test";
import assert from "node:assert/strict";
import { linkView } from "./link.ts";

test("the panel shows the ping; the quality is in the bars and the tooltip", () => {
  assert.deepEqual(linkView({ quality: "excellent", rttMs: 10 }), {
    text: "10 ms",
    quality: "Отличная связь",
    bars: 3,
    tone: "good",
  });
  assert.equal(linkView({ quality: "poor", rttMs: 180 }).tone, "weak");
  assert.equal(linkView({ quality: "excellent", rttMs: 0 }).text, "<1 ms", "a LAN server");
  assert.deepEqual(linkView({ quality: "lost", rttMs: 300 }), {
    text: "Нет связи",
    quality: "Связь потеряна",
    bars: 0,
    tone: "bad",
  });
});

test("bars stay grey until measured and while the server is out of reach", () => {
  assert.deepEqual(linkView(null), { text: "Подключение…", quality: "Связь проверяется", bars: 0, tone: "pending" });
  const unmeasured = linkView({ quality: "excellent", rttMs: null });
  assert.equal(unmeasured.tone, "pending", "LiveKit's rating alone is no measurement yet");
  assert.equal(unmeasured.text, "Подключение…");
  const offline = linkView({ quality: "excellent", rttMs: 12 }, false);
  assert.equal(offline.tone, "pending");
  assert.equal(offline.quality, "Нет связи с сервером");
});
