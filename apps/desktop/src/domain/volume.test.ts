import { test } from "node:test";
import assert from "node:assert/strict";
import { clampVolume, DEFAULT_PREFS, VolumeBook, type KeyValueStorage } from "./volume.ts";

class MemoryStorage implements KeyValueStorage {
  readonly data = new Map<string, string>();
  getItem(key: string): string | null {
    return this.data.get(key) ?? null;
  }
  setItem(key: string, value: string): void {
    this.data.set(key, value);
  }
}

test("clampVolume keeps 0–200 and rounds", () => {
  assert.equal(clampVolume(150.4), 150);
  assert.equal(clampVolume(-10), 0);
  assert.equal(clampVolume(999), 200);
  assert.equal(clampVolume(Number.NaN), 100);
});

test("unknown users get the default", () => {
  const book = new VolumeBook(new MemoryStorage());
  assert.deepEqual(book.get("lena"), DEFAULT_PREFS);
});

test("volume and mute persist across instances", () => {
  const storage = new MemoryStorage();
  const book = new VolumeBook(storage);
  book.setVolume("max", 40);
  book.setMuted("max", true);
  const reloaded = new VolumeBook(storage);
  assert.deepEqual(reloaded.get("max"), { volume: 40, muted: true });
});

test("muting keeps the volume for later", () => {
  const book = new VolumeBook(new MemoryStorage());
  book.setVolume("max", 160);
  book.setMuted("max", true);
  assert.deepEqual(book.setMuted("max", false), { volume: 160, muted: false });
});

test("default prefs are not stored", () => {
  const storage = new MemoryStorage();
  const book = new VolumeBook(storage);
  book.setVolume("artem", 120);
  book.setVolume("artem", 100);
  assert.equal(storage.getItem("ourcamp.volumes.v1"), "{}");
});

test("corrupt or foreign storage is ignored safely", () => {
  for (const raw of ["not json", "[1,2]", "null", '{"a": 5, "b": {"volume": "x", "muted": 1}, "c": {"volume": 900}}']) {
    const storage = new MemoryStorage();
    storage.setItem("ourcamp.volumes.v1", raw);
    const book = new VolumeBook(storage);
    assert.deepEqual(book.get("a"), DEFAULT_PREFS, raw);
    if (raw.startsWith('{"a"')) {
      assert.deepEqual(book.get("b"), { volume: 100, muted: false });
      assert.deepEqual(book.get("c"), { volume: 200, muted: false });
    }
  }
});
