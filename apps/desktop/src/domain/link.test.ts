import { test } from "node:test";
import assert from "node:assert/strict";
import { linkView } from "./link.ts";

test("the panel says how the connection is, with the ping once measured", () => {
  assert.deepEqual(linkView({ quality: "excellent", rttMs: 24 }), {
    label: "Отличная связь",
    ping: "24 мс",
    bars: 3,
    tone: "good",
  });
  assert.equal(linkView({ quality: "good", rttMs: null }).ping, null, "not measured yet");
  assert.equal(linkView({ quality: "poor", rttMs: 180 }).tone, "weak");
  assert.equal(linkView({ quality: "excellent", rttMs: 0 }).ping, "<1 мс", "a LAN server");
  assert.deepEqual(linkView({ quality: "lost", rttMs: 300 }), {
    label: "Связь потеряна",
    ping: null,
    bars: 0,
    tone: "bad",
  });
  assert.equal(linkView(null).label, "Подключение…");
});
