import assert from "node:assert/strict";
import { test } from "node:test";

import { dropTarget, pastThreshold } from "./drag.ts";

test("a drop moves only to another channel", () => {
  assert.equal(dropTarget("c0", "c1"), "c1");
  assert.equal(dropTarget("c0", "c0"), null, "already sitting there: nothing to do");
  assert.equal(dropTarget("c0", null), null, "dropped outside any channel");
});

test("a small wobble is still a click", () => {
  assert.equal(pastThreshold(2, 3), false);
  assert.equal(pastThreshold(3, 4), true);
  assert.equal(pastThreshold(-6, 0), true);
});
