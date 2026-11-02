// A sample scenario (ADR-0078): the app started with a link fills in the form, joins nothing by
// itself, and a press gets into the camp. A second launch is not covered here: `single-instance`
// is off under OC_PROFILE, it was checked with `if profile().is_none()` patched for a moment.
import assert from "node:assert/strict";
import { LINK, ev, fill, mainWindows, pressButton, startClient, until, wait } from "./cdp.mjs";

const FIELDS = "input:not([type=file])";
const client = startClient("check-a", [LINK]);
try {
  const [main] = await mainWindows(1);
  await until(main, `document.querySelectorAll(${JSON.stringify(FIELDS)}).length>=2`, "the join form");
  assert.equal(await ev(main, `document.querySelector(${JSON.stringify(FIELDS)}).value`), LINK);

  await wait(2000);
  assert.equal(await ev(main, `document.querySelectorAll(".channel__row").length`), 0, "joined by itself");

  await ev(main, fill(FIELDS, 1, "Аня"));
  await wait(1000);
  assert.ok(await ev(main, pressButton("Войти на сервер")), "no join button");
  await until(main, `document.querySelectorAll(".channel__row").length>0`, "the camp's channels");
  console.log("open-link: ok");
} finally {
  client.kill();
}
process.exit(0);
