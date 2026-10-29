import assert from "node:assert/strict";
import { test } from "node:test";

import { addressEntries, addressHint, choiceFromValue, choiceValue, type Reach } from "./address.ts";

const reach = (over: Partial<Reach> = {}): Reach => ({
  choice: { kind: "auto" },
  options: [
    { kind: "lan", ip: "192.168.1.2" },
    { kind: "radmin", ip: "26.12.34.56" },
  ],
  note: null,
  ...over,
});

test("the list offers automatic, each network found, and one's own", () => {
  assert.deepEqual(
    addressEntries(reach()).map((e) => [e.label, e.detail]),
    [
      ["Автоматически", "192.168.1.2"],
      ["Локальная сеть", "192.168.1.2"],
      ["Radmin VPN", "26.12.34.56"],
      ["Свой адрес: домен или IP", undefined],
    ],
  );
  const gone = addressEntries(reach({ choice: { kind: "detected", which: "hamachi" } }));
  assert.ok(
    gone.some((e) => e.label === "Hamachi" && e.detail === "не найден"),
    "a kept choice that is gone stays visible",
  );
});

test("each choice says what friends need", () => {
  assert.match(addressHint(reach()), /той же сети: один роутер/);
  assert.match(addressHint(reach({ choice: { kind: "detected", which: "radmin" } })), /Radmin VPN. Порты открывать не нужно/);
  assert.match(addressHint(reach({ choice: { kind: "custom", host: "x" } })), /7882 \(UDP\)/);
  assert.match(addressHint(reach({ options: [{ kind: "public", ip: "85.1.2.3" }] })), /из интернета/, "a hosting server");
});

test("choices go through the select and back", () => {
  for (const choice of [{ kind: "auto" }, { kind: "detected", which: "tailscale" }] as const) {
    assert.deepEqual(choiceFromValue(choiceValue(choice), ""), choice);
  }
  assert.deepEqual(choiceFromValue("custom", "mycamp.duckdns.org"), { kind: "custom", host: "mycamp.duckdns.org" });
});
