// Drives the real app's windows over WebView2's debug port (ADR-0047). For scenarios run by
// `app.sh`, which passes the camp's link in OC_LINK and the client in OC_EXE.
import { spawn } from "node:child_process";

const PORT = 9333;

export const LINK = process.env.OC_LINK;
export const wait = (ms) => new Promise((r) => setTimeout(r, ms));

/** Starts a client as another person on this PC. Two clients share one debug port. */
export function startClient(profile, args = []) {
  return spawn(process.env.OC_EXE, args, {
    env: {
      ...process.env,
      OC_PROFILE: profile,
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${PORT}`,
    },
    stdio: "ignore",
  });
}

export async function pages() {
  const answer = await fetch(`http://127.0.0.1:${PORT}/json`);
  return (await answer.json()).filter((t) => t.type === "page");
}

/** The main windows of the clients, once `count` of them are up. */
export async function mainWindows(count, seconds = 40) {
  for (let i = 0; i < seconds * 2; i++) {
    const mains = await pages().then(
      (all) => all.filter((p) => !/settings|chat|stream/.test(p.url)),
      () => [],
    );
    if (mains.length === count) return mains;
    await wait(500);
  }
  throw new Error(`no ${count} main windows after ${seconds} s`);
}

async function rpc(target, method, params) {
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((ok, no) => {
    ws.onopen = ok;
    ws.onerror = no;
  });
  const reply = new Promise((ok) => {
    ws.onmessage = (m) => {
      const d = JSON.parse(m.data);
      if (d.id === 1) ok(d.result);
    };
  });
  ws.send(JSON.stringify({ id: 1, method, params }));
  const answer = await Promise.race([reply, wait(60000).then(() => null)]);
  ws.close();
  if (answer === null) throw new Error(`${method}: no answer in 60 s`);
  return answer;
}

/** Evaluates `expression` in the window; promises are awaited. */
export async function ev(target, expression) {
  const r = await rpc(target, "Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description ?? "evaluation failed");
  return r.result.value;
}

/** Waits until `expression` is truthy in the window. */
export async function until(target, expression, what, seconds = 15) {
  for (let i = 0; i < seconds * 5; i++) {
    const value = await ev(target, expression);
    if (value) return value;
    await wait(200);
  }
  throw new Error(`not in ${seconds} s: ${what}`);
}

export const pressButton = (text) =>
  `(()=>{const b=[...document.querySelectorAll("button")].find(b=>b.textContent.includes(${JSON.stringify(text)}));b?.click();return !!b;})()`;

/** Sets an input the way typing does, so React sees it. */
export const fill = (selector, index, value) =>
  `(()=>{const el=document.querySelectorAll(${JSON.stringify(selector)})[${index}];Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,"value").set.call(el,${JSON.stringify(value)});el.dispatchEvent(new Event("input",{bubbles:true}));return 1;})()`;
