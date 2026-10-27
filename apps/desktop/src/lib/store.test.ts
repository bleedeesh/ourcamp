import { test } from "node:test";
import assert from "node:assert/strict";
import { createStore } from "./store.ts";

test("patch merges and notifies once", () => {
  const store = createStore({ a: 1, b: "x" });
  let calls = 0;
  store.subscribe(() => calls++);
  store.patch({ a: 2 });
  assert.deepEqual(store.get(), { a: 2, b: "x" });
  assert.equal(calls, 1);
});

test("patch with identical values does not notify or replace state", () => {
  const store = createStore({ a: 1, b: "x" });
  const before = store.get();
  let calls = 0;
  store.subscribe(() => calls++);
  store.patch({ a: 1 });
  assert.equal(calls, 0);
  assert.equal(store.get(), before);
});

test("update returning the same state is a no-op", () => {
  const store = createStore({ n: 0 });
  let calls = 0;
  store.subscribe(() => calls++);
  store.update((s) => s);
  store.update((s) => ({ n: s.n + 1 }));
  assert.equal(calls, 1);
  assert.equal(store.get().n, 1);
});

test("unsubscribe stops notifications, even during notification", () => {
  const store = createStore({ n: 0 });
  const seen: string[] = [];
  const offA = store.subscribe(() => {
    seen.push("a");
    offB();
  });
  const offB = store.subscribe(() => seen.push("b"));
  store.patch({ n: 1 });
  store.patch({ n: 2 });
  offA();
  store.patch({ n: 3 });
  assert.deepEqual(seen, ["a", "b", "a"]);
});
