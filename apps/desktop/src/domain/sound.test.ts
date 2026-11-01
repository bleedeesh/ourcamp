import assert from "node:assert/strict";
import { test } from "node:test";

import { AS_IN_SYSTEM, deviceChoice, deviceOptions, deviceValue } from "./sound.ts";

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
