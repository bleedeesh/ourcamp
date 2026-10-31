import assert from "node:assert/strict";
import { test } from "node:test";
import type { CampState } from "../engine/camp-api.ts";
import {
  bitrateOf,
  channelNow,
  columnOf,
  heavyText,
  keepChoice,
  mbitText,
  soundHint,
  sourceTiles,
  uploadText,
  type QualityOption,
  type StreamSource,
} from "./stream-start.ts";

const SOURCES: StreamSource[] = [
  { kind: "window", id: 7, title: " Игра " },
  { kind: "screen", id: 0, title: "" },
  { kind: "window", id: 8, title: "   " },
  { kind: "screen", id: 1, title: "" },
];

const OPTIONS: QualityOption[] = [
  { height: 720, fps: 30, bitrate: 2_500_000 },
  { height: 720, fps: 60, bitrate: 4_000_000 },
  { height: 1440, fps: 30, bitrate: 7_000_000 },
  { height: 1440, fps: 60, bitrate: 10_000_000 },
];

function state(presence: CampState["presence"]): CampState {
  return {
    name: "Лагерь",
    channels: [],
    members: [
      { userId: "me", name: "Я", role: "member", silenced: false },
      { userId: "lena", name: "Лена", role: "member", silenced: false },
    ],
    presence,
  };
}

test("screens come by number, windows by title, and untitled windows stay out", () => {
  assert.deepEqual(
    sourceTiles(SOURCES).map((t) => [t.key, t.label]),
    [
      ["screen:0", "Весь экран 1"],
      ["screen:1", "Весь экран 2"],
      ["window:7", "Окно: Игра"],
    ],
  );
});

test("the choice lasts as long as its source, then falls to the first", () => {
  const tiles = sourceTiles(SOURCES);
  assert.equal(keepChoice(tiles, "window:7"), "window:7");
  assert.equal(keepChoice(tiles, "window:99"), "screen:0");
  assert.equal(keepChoice(tiles, null), "screen:0");
  assert.equal(keepChoice([], "screen:0"), null);
});

test("the table gives its columns and what a quality costs", () => {
  assert.deepEqual(columnOf(OPTIONS, "height"), [720, 1440]);
  assert.deepEqual(columnOf(OPTIONS, "fps"), [30, 60]);
  assert.equal(bitrateOf(OPTIONS, 1440, 30), 7_000_000);
  assert.equal(bitrateOf(OPTIONS, 480, 30), 0);
});

test("your channel: who else is there, and who of them is on air", () => {
  const now = channelNow(
    state([
      { channelId: "other", members: [{ userId: "pasha", micMuted: false, streaming: true }] },
      {
        channelId: "mine",
        members: [
          { userId: "me", micMuted: false, streaming: true },
          { userId: "lena", micMuted: false, streaming: true },
          { userId: "ghost", micMuted: false },
        ],
      },
    ]),
    "me",
  );
  assert.equal(now.others, 2);
  assert.deepEqual(now.onAir, [{ userId: "lena", name: "Лена" }]);
});

test("not in any channel yet, or no snapshot: nobody around", () => {
  assert.deepEqual(channelNow(null, "me"), { others: 0, onAir: [] });
  assert.deepEqual(channelNow(state([{ channelId: "x", members: [{ userId: "lena", micMuted: false }] }]), "me"), {
    others: 0,
    onAir: [],
  });
});

test("the upload warning counts everyone who may watch", () => {
  assert.equal(mbitText(2_500_000), "2,5");
  assert.equal(mbitText(10_000_000), "10");
  assert.match(uploadText(6_000_000, 0), /до 6 Мбит\/с\. На домашнем сервере это исходящий канал хозяина\.$/);
  assert.match(uploadText(6_000_000, 3), /При 3 зрителях это до 18 Мбит\/с/);
  assert.match(uploadText(2_500_000, 1), /При 1 зрителе это до 2,5 Мбит\/с/);
  assert.match(uploadText(2_500_000, 11), /При 11 зрителях/);
  assert.match(uploadText(2_500_000, 21), /При 21 зрителе/);
});

test("the hint says whose sound goes out", () => {
  assert.match(soundHint("screen"), /кроме голосов/);
  assert.match(soundHint("window"), /всего приложения/);
});

test("only 1440p warns about the load", () => {
  assert.equal(heavyText(1080), null);
  assert.match(heavyText(1440) ?? "", /^1440p/);
});
