import assert from "node:assert/strict";
import { test } from "node:test";

import { addressEntries, addressHint, choiceFromValue, choiceValue, portsLine, type Reach } from "./address.ts";

const reach = (over: Partial<Reach> = {}): Reach => ({
  choice: { kind: "auto" },
  options: [
    { kind: "lan", ip: "192.168.1.2", grey: false },
    { kind: "radmin", ip: "26.12.34.56", grey: false },
  ],
  note: null,
  ports: null,
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

/** The hint as text: a missing one matches nothing. */
const hint = (r: Reach) => addressHint(r) ?? "";

test("each choice says what friends need", () => {
  assert.match(hint(reach()), /той же сети: один роутер/);
  assert.match(hint(reach({ choice: { kind: "detected", which: "radmin" } })), /Radmin VPN. Порты открывать не нужно/);
  assert.match(hint(reach({ choice: { kind: "custom", host: "x" } })), /7882 \(UDP\)/);
  assert.match(hint(reach({ options: [{ kind: "public", ip: "85.1.2.3", grey: false }] })), /из интернета/, "a hosting server");
  assert.match(hint(reach({ choice: { kind: "detected", which: "router" } })), /откроет сам \(UPnP\)/);
});

test("choices go through the select and back", () => {
  for (const choice of [{ kind: "auto" }, { kind: "detected", which: "tailscale" }] as const) {
    assert.deepEqual(choiceFromValue(choiceValue(choice), ""), choice);
  }
  assert.deepEqual(choiceFromValue("custom", "mycamp.duckdns.org"), { kind: "custom", host: "mycamp.duckdns.org" });
});

test("a grey router address is marked in the list", () => {
  const entries = addressEntries(reach({ options: [{ kind: "router", ip: "100.91.68.10", grey: true }] }));
  assert.ok(entries.some((e) => e.label === "Интернет через роутер" && e.detail === "100.91.68.10 · серый"));
  const grey = reach({ choice: { kind: "detected", which: "router" }, options: [{ kind: "router", ip: "100.91.68.10", grey: true }] });
  assert.equal(addressHint(grey), null, "the warning says it all");
});

test("the ports line says what is known, and only that", () => {
  assert.equal(portsLine(null), null, "nothing asked of the router");
  assert.equal(portsLine({ state: "opening" })?.warn, false);
  assert.match(portsLine({ state: "open", externalIp: "85.1.2.3", answers: true })!.text, /отвечает по 85.1.2.3/);
  assert.match(portsLine({ state: "open", externalIp: "85.1.2.3", answers: null })!.text, /Попросите друга/, "no loopback: no claim");
  const failed = portsLine({ state: "failed", message: "порт 7780 на роутере уже занят другим устройством" })!;
  assert.equal(failed.warn, true);
  assert.match(failed.text, /вручную/);
});
