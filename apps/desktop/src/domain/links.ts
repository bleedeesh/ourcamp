/** A piece of a message's text: `href` when it is a link. */
export interface TextPart {
  readonly text: string;
  readonly href?: string;
}

/** A link as a card names it (`oc_proto::LinkCard`): the text shows it, all but a `www.` link's scheme. */
export interface FoundLink {
  readonly url: string;
}

/** The longest address of a card (`oc_proto::MAX_LINK_URL_BYTES`). */
const MAX_URL_BYTES = 2048;

/** Starts with a scheme or `www.`; runs to whitespace or what never stands in an address. */
const CANDIDATE = /(?<![\p{L}\p{N}])(?:https?:\/\/|www\.)[^\s<>"«»]+/giu;
/** Punctuation that ends a sentence, not an address. */
const TRAILING = /[.,;:!?…'*_~]+$/u;
const BRACKETS: ReadonlyArray<readonly [open: string, close: string]> = [
  ["(", ")"],
  ["[", "]"],
  ["{", "}"],
];

const count = (text: string, char: string) => text.split(char).length - 1;

/** Without what follows the address: a sentence's end, a bracket opened before it. */
function trimmed(candidate: string): string {
  let text = candidate;
  for (;;) {
    const before = text;
    text = text.replace(TRAILING, "");
    for (const [open, close] of BRACKETS) {
      if (text.endsWith(close) && count(text, close) > count(text, open)) text = text.slice(0, -1);
    }
    if (text === before) return text;
  }
}

/** The address to open: `null` for what must not count as a link, a login in it included. */
function webAddress(written: string): string | null {
  try {
    const url = new URL(/^www\./i.test(written) ? `https://${written}` : written);
    const web = url.protocol === "http:" || url.protocol === "https:";
    if (!web || url.username !== "" || url.password !== "" || !url.hostname.includes(".")) return null;
    return url.href;
  } catch {
    return null;
  }
}

/** The text, its links marked. Whatever is not surely a link stays text. */
export function splitLinks(text: string): TextPart[] {
  const parts: TextPart[] = [];
  let at = 0;
  for (const match of text.matchAll(CANDIDATE)) {
    const written = trimmed(match[0]);
    const href = webAddress(written);
    if (href === null) continue;
    if (match.index > at) parts.push({ text: text.slice(at, match.index) });
    parts.push({ text: written, href });
    at = match.index + written.length;
  }
  if (at < text.length) parts.push({ text: text.slice(at) });
  return parts;
}

/** The first link a card can be made of, as the camp will check it. */
export function firstLink(text: string): FoundLink | null {
  for (const part of splitLinks(text)) {
    if (part.href === undefined) continue;
    const url = /^www\./i.test(part.text) ? `https://${part.text}` : part.text;
    if (new TextEncoder().encode(url).length > MAX_URL_BYTES) continue;
    // Only in lower case does the camp know a scheme.
    return { url: url.replace(/^https?:\/\//i, (scheme) => scheme.toLowerCase()) };
  }
  return null;
}

/** The site's name a card shows: from the address itself, never from the page's words. */
export function siteOf(url: string): string {
  try {
    return new URL(url).hostname.replace(/^www\./, "");
  } catch {
    return "";
  }
}
