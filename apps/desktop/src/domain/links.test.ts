import { test } from "node:test";
import assert from "node:assert/strict";
import { firstLink, siteOf, splitLinks } from "./links.ts";

const links = (text: string) => splitLinks(text).flatMap((p) => (p.href === undefined ? [] : [p.text]));

test("a link is found among words, and the text stays whole", () => {
  assert.deepEqual(splitLinks("смотри https://example.com/a?b=1#c тут"), [
    { text: "смотри " },
    { text: "https://example.com/a?b=1#c", href: "https://example.com/a?b=1#c" },
    { text: " тут" },
  ]);
  assert.deepEqual(splitLinks("просто текст"), [{ text: "просто текст" }]);
  assert.deepEqual(splitLinks(""), []);
  const text = "а www.example.com б http://example.org в";
  assert.equal(
    splitLinks(text)
      .map((p) => p.text)
      .join(""),
    text,
  );
  assert.equal(splitLinks("www.example.com")[0]?.href, "https://www.example.com/");
});

test("what follows an address is not part of it", () => {
  assert.deepEqual(links("вот https://example.com."), ["https://example.com"]);
  assert.deepEqual(links("(https://example.com)"), ["https://example.com"]);
  assert.deepEqual(links("https://example.com, https://example.org!"), ["https://example.com", "https://example.org"]);
  assert.deepEqual(links("«https://example.com»"), ["https://example.com"]);
  assert.deepEqual(links("https://example.com/a_(b)"), ["https://example.com/a_(b)"], "its own bracket stays");
});

test("addresses in other alphabets are links too", () => {
  const [part] = splitLinks("https://ru.wikipedia.org/wiki/Костёр");
  assert.equal(part?.text, "https://ru.wikipedia.org/wiki/Костёр");
  assert.equal(part?.href, "https://ru.wikipedia.org/wiki/%D0%9A%D0%BE%D1%81%D1%82%D1%91%D1%80");
});

test("nothing that misleads or is not of the web becomes a link", () => {
  for (const text of [
    "javascript:alert(1)",
    "file:///C:/Windows",
    "ftp://example.com",
    "https://bank.example@evil.example/",
    "https://user:pass@example.com",
    "https://",
    "https://localhost/",
    "www.",
    "нехttps://example.com",
    "example.com",
  ]) {
    assert.deepEqual(links(text), [], text);
  }
});

test("a card names the first link as it is written", () => {
  assert.deepEqual(firstLink("а https://a.example/Б и https://b.example"), { url: "https://a.example/Б" });
  assert.deepEqual(firstLink("www.example.com"), { url: "https://www.example.com" });
  assert.deepEqual(firstLink("HTTPS://Example.com/A"), { url: "https://Example.com/A" });
  assert.equal(firstLink("без ссылок"), null);
  assert.equal(firstLink(`https://example.com/${"a".repeat(2048)}`), null, "too long for a card");
});

test("the site's name comes from the address", () => {
  assert.equal(siteOf("https://www.youtube.com/watch?v=1"), "youtube.com");
  assert.equal(siteOf("https://bank.example.evil.example/"), "bank.example.evil.example");
  assert.equal(siteOf("не адрес"), "");
});
