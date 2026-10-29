// Generates packages/ui/src/icon-shapes.tsx from Lucide SVGs (ADR-0027).
// node packages/ui/scripts/gen-icons.mjs <dir with lucide-static@1.56.0 icons/*.svg> packages/ui/src/icon-shapes.tsx
// The SVGs come from https://unpkg.com/lucide-static@1.56.0/icons/<name>.svg; a new icon is
// a line in ICONS below (its Lucide name and which shapes the solid style fills).
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const [dir, out] = process.argv.slice(2);

// Our name → Lucide name, and the shapes filled in the solid style (ADR-0027).
const ICONS = {
  mic: ["mic", '<rect x="9" y="2" width="6" height="13" rx="3" />'],
  micOff: ["mic-off", null],
  headphones: ["headphones", '<rect x="3" y="14" width="5" height="7" rx="2" /><rect x="16" y="14" width="5" height="7" rx="2" />'],
  headphonesOff: ["headphone-off", '<rect x="3" y="14" width="5" height="7" rx="2" />'],
  leave: ["log-out", null],
  gear: [
    "settings",
    '<path d="M9.671 4.136a2.34 2.34 0 0 1 4.659 0 2.34 2.34 0 0 0 3.319 1.915 2.34 2.34 0 0 1 2.33 4.033 2.34 2.34 0 0 0 0 3.831 2.34 2.34 0 0 1-2.33 4.033 2.34 2.34 0 0 0-3.319 1.915 2.34 2.34 0 0 1-4.659 0 2.34 2.34 0 0 0-3.32-1.915 2.34 2.34 0 0 1-2.33-4.033 2.34 2.34 0 0 0 0-3.831A2.34 2.34 0 0 1 6.35 6.051a2.34 2.34 0 0 0 3.319-1.915M15 12a3 3 0 1 0-6 0 3 3 0 1 0 6 0" />',
  ],
  chat: ["message-square", "same"],
  screen: ["monitor", '<rect width="20" height="14" x="2" y="3" rx="2" />'],
  users: ["users", '<path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2z" /><circle cx="9" cy="7" r="4" />'],
  chevron: ["chevron-down", null],
  back: ["chevron-left", null],
  minimize: ["minus", null],
  close: ["x", null],
  crown: ["crown", "first"],
  moon: ["moon", "same"],
  shield: ["shield", "same"],
  speaker: ["volume-2", "first"],
  speakerOff: ["volume-x", "first"],
  reset: ["rotate-ccw", null],
  check: ["check", null],
  plus: ["plus", null],
  link: ["link", null],
  paste: ["clipboard-paste", '<rect x="8" y="2" width="8" height="4" rx="1" />'],
  copy: ["copy", '<rect width="14" height="14" x="8" y="8" rx="2" ry="2" />'],
  pencil: ["pencil", "first"],
  trash: ["trash-2", null],
  hash: ["hash", null],
  move: ["arrow-right-left", null],
  kick: ["user-x", '<path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2z" /><circle cx="9" cy="7" r="4" />'],
};

const shapes = (lucide) => {
  const svg = readFileSync(join(dir, `${lucide}.svg`), "utf8");
  return [...svg.matchAll(/<(path|rect|circle|ellipse|line|polyline|polygon)\b[^>]*\/>/g)].map((m) => m[0].replace(/\s+/g, " "));
};

let body = "";
for (const [name, [lucide, fill]] of Object.entries(ICONS)) {
  const strokes = shapes(lucide);
  let solid = fill;
  if (fill === "same") solid = strokes.join("");
  if (fill === "first") solid = strokes[0];
  body += `  ${name}: {\n    lucide: "${lucide}",\n    stroke: (\n      <>\n        ${strokes.join("\n        ")}\n      </>\n    ),\n`;
  if (solid) body += `    fill: (\n      <>\n        ${solid.replace(/></g, ">\n        <")}\n      </>\n    ),\n`;
  body += "  },\n";
}

writeFileSync(
  out,
  `// Generated from Lucide 1.56.0 (ISC, see LICENSE-lucide.txt) with the solid fills of ADR-0027.
// Regenerate rather than hand-edit the stroke shapes.
import type { ReactNode } from "react";

export interface IconShape {
  /** Lucide icon this comes from. */
  readonly lucide: string;
  /** Lucide's own outline, drawn on top. */
  readonly stroke: ReactNode;
  /** Closed parts filled solid, under the outline (fill-rule evenodd: holes stay holes). */
  readonly fill?: ReactNode;
}

export const ICONS = {
${body}} as const satisfies Record<string, IconShape>;
`,
);
console.log(`${Object.keys(ICONS).length} icons → ${out}`);
