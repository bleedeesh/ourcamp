import assert from "node:assert/strict";
import { test } from "node:test";

import { nativeMenuWanted } from "./context-menu.ts";

/** An element that is (or sits in) a field, or not. */
const el = (inField: boolean) => ({ closest: () => (inField ? {} : null) });

test("the webview's own menu stays only for fields and selected text", () => {
  assert.equal(nativeMenuWanted(el(false), ""), false, "a channel, a button, empty space");
  assert.equal(nativeMenuWanted(el(true), ""), true, "paste into the link field");
  assert.equal(nativeMenuWanted(el(false), "ourcamp://…"), true, "copy a selected link");
  assert.equal(nativeMenuWanted(el(false), "  \n"), false, "a stray click-selection is no text");
  assert.equal(nativeMenuWanted(null, ""), false);
});
