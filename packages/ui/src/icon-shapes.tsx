// Generated from Lucide 1.56.0 (ISC, see LICENSE-lucide.txt) with the solid fills of ADR-0027.
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
  mic: {
    lucide: "mic",
    stroke: (
      <>
        <path d="M12 19v3" />
        <path d="M19 10v2a7 7 0 0 1-14 0v-2" />
        <rect x="9" y="2" width="6" height="13" rx="3" />
      </>
    ),
    fill: (
      <>
        <rect x="9" y="2" width="6" height="13" rx="3" />
      </>
    ),
  },
  micOff: {
    lucide: "mic-off",
    stroke: (
      <>
        <path d="M12 19v3" />
        <path d="M15 9.34V5a3 3 0 0 0-5.68-1.33" />
        <path d="M16.95 16.95A7 7 0 0 1 5 12v-2" />
        <path d="M18.89 13.23A7 7 0 0 0 19 12v-2" />
        <path d="m2 2 20 20" />
        <path d="M9 9v3a3 3 0 0 0 5.12 2.12" />
      </>
    ),
  },
  headphones: {
    lucide: "headphones",
    stroke: (
      <>
        <path d="M3 14h3a2 2 0 0 1 2 2v3a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-7a9 9 0 0 1 18 0v7a2 2 0 0 1-2 2h-1a2 2 0 0 1-2-2v-3a2 2 0 0 1 2-2h3" />
      </>
    ),
    fill: (
      <>
        <rect x="3" y="14" width="5" height="7" rx="2" />
        <rect x="16" y="14" width="5" height="7" rx="2" />
      </>
    ),
  },
  headphonesOff: {
    lucide: "headphone-off",
    stroke: (
      <>
        <path d="M21 14h-1.343" />
        <path d="M9.128 3.47A9 9 0 0 1 21 12v3.343" />
        <path d="m2 2 20 20" />
        <path d="M20.414 20.414A2 2 0 0 1 19 21h-1a2 2 0 0 1-2-2v-3" />
        <path d="M3 14h3a2 2 0 0 1 2 2v3a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-7a9 9 0 0 1 2.636-6.364" />
      </>
    ),
    fill: (
      <>
        <rect x="3" y="14" width="5" height="7" rx="2" />
      </>
    ),
  },
  leave: {
    lucide: "log-out",
    stroke: (
      <>
        <path d="m16 17 5-5-5-5" />
        <path d="M21 12H9" />
        <path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4" />
      </>
    ),
  },
  gear: {
    lucide: "settings",
    stroke: (
      <>
        <path d="M9.671 4.136a2.34 2.34 0 0 1 4.659 0 2.34 2.34 0 0 0 3.319 1.915 2.34 2.34 0 0 1 2.33 4.033 2.34 2.34 0 0 0 0 3.831 2.34 2.34 0 0 1-2.33 4.033 2.34 2.34 0 0 0-3.319 1.915 2.34 2.34 0 0 1-4.659 0 2.34 2.34 0 0 0-3.32-1.915 2.34 2.34 0 0 1-2.33-4.033 2.34 2.34 0 0 0 0-3.831A2.34 2.34 0 0 1 6.35 6.051a2.34 2.34 0 0 0 3.319-1.915" />
        <circle cx="12" cy="12" r="3" />
      </>
    ),
    fill: (
      <>
        <path d="M9.671 4.136a2.34 2.34 0 0 1 4.659 0 2.34 2.34 0 0 0 3.319 1.915 2.34 2.34 0 0 1 2.33 4.033 2.34 2.34 0 0 0 0 3.831 2.34 2.34 0 0 1-2.33 4.033 2.34 2.34 0 0 0-3.319 1.915 2.34 2.34 0 0 1-4.659 0 2.34 2.34 0 0 0-3.32-1.915 2.34 2.34 0 0 1-2.33-4.033 2.34 2.34 0 0 0 0-3.831A2.34 2.34 0 0 1 6.35 6.051a2.34 2.34 0 0 0 3.319-1.915M15 12a3 3 0 1 0-6 0 3 3 0 1 0 6 0" />
      </>
    ),
  },
  chat: {
    lucide: "message-square",
    stroke: (
      <>
        <path d="M22 17a2 2 0 0 1-2 2H6.828a2 2 0 0 0-1.414.586l-2.202 2.202A.71.71 0 0 1 2 21.286V5a2 2 0 0 1 2-2h16a2 2 0 0 1 2 2z" />
      </>
    ),
    fill: (
      <>
        <path d="M22 17a2 2 0 0 1-2 2H6.828a2 2 0 0 0-1.414.586l-2.202 2.202A.71.71 0 0 1 2 21.286V5a2 2 0 0 1 2-2h16a2 2 0 0 1 2 2z" />
      </>
    ),
  },
  screen: {
    lucide: "monitor",
    stroke: (
      <>
        <rect width="20" height="14" x="2" y="3" rx="2" />
        <line x1="8" x2="16" y1="21" y2="21" />
        <line x1="12" x2="12" y1="17" y2="21" />
      </>
    ),
    fill: (
      <>
        <rect width="20" height="14" x="2" y="3" rx="2" />
      </>
    ),
  },
  users: {
    lucide: "users",
    stroke: (
      <>
        <path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2" />
        <path d="M16 3.128a4 4 0 0 1 0 7.744" />
        <path d="M22 21v-2a4 4 0 0 0-3-3.87" />
        <circle cx="9" cy="7" r="4" />
      </>
    ),
    fill: (
      <>
        <path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2z" />
        <circle cx="9" cy="7" r="4" />
      </>
    ),
  },
  chevron: {
    lucide: "chevron-down",
    stroke: (
      <>
        <path d="m6 9 6 6 6-6" />
      </>
    ),
  },
  back: {
    lucide: "chevron-left",
    stroke: (
      <>
        <path d="m15 18-6-6 6-6" />
      </>
    ),
  },
  minimize: {
    lucide: "minus",
    stroke: (
      <>
        <path d="M5 12h14" />
      </>
    ),
  },
  close: {
    lucide: "x",
    stroke: (
      <>
        <path d="M18 6 6 18" />
        <path d="m6 6 12 12" />
      </>
    ),
  },
  crown: {
    lucide: "crown",
    stroke: (
      <>
        <path d="M11.562 3.266a.5.5 0 0 1 .876 0L15.39 8.87a1 1 0 0 0 1.516.294L21.183 5.5a.5.5 0 0 1 .798.519l-2.834 10.246a1 1 0 0 1-.956.734H5.81a1 1 0 0 1-.957-.734L2.02 6.02a.5.5 0 0 1 .798-.519l4.276 3.664a1 1 0 0 0 1.516-.294z" />
        <path d="M5 21h14" />
      </>
    ),
    fill: (
      <>
        <path d="M11.562 3.266a.5.5 0 0 1 .876 0L15.39 8.87a1 1 0 0 0 1.516.294L21.183 5.5a.5.5 0 0 1 .798.519l-2.834 10.246a1 1 0 0 1-.956.734H5.81a1 1 0 0 1-.957-.734L2.02 6.02a.5.5 0 0 1 .798-.519l4.276 3.664a1 1 0 0 0 1.516-.294z" />
      </>
    ),
  },
  moon: {
    lucide: "moon",
    stroke: (
      <>
        <path d="M20.985 12.486a9 9 0 1 1-9.473-9.472c.405-.022.617.46.402.803a6 6 0 0 0 8.268 8.268c.344-.215.825-.004.803.401" />
      </>
    ),
    fill: (
      <>
        <path d="M20.985 12.486a9 9 0 1 1-9.473-9.472c.405-.022.617.46.402.803a6 6 0 0 0 8.268 8.268c.344-.215.825-.004.803.401" />
      </>
    ),
  },
  shield: {
    lucide: "shield",
    stroke: (
      <>
        <path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z" />
      </>
    ),
    fill: (
      <>
        <path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z" />
      </>
    ),
  },
  speaker: {
    lucide: "volume-2",
    stroke: (
      <>
        <path d="M11 4.702a.705.705 0 0 0-1.203-.498L6.413 7.587A1.4 1.4 0 0 1 5.416 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.705.705 0 0 0 11 19.298z" />
        <path d="M16 9a5 5 0 0 1 0 6" />
        <path d="M19.364 18.364a9 9 0 0 0 0-12.728" />
      </>
    ),
    fill: (
      <>
        <path d="M11 4.702a.705.705 0 0 0-1.203-.498L6.413 7.587A1.4 1.4 0 0 1 5.416 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.705.705 0 0 0 11 19.298z" />
      </>
    ),
  },
  speakerOff: {
    lucide: "volume-x",
    stroke: (
      <>
        <path d="M11 4.702a.7.7 0 0 0-1.203-.498L6.413 7.587A1.4 1.4 0 0 1 5.416 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.7.7 0 0 0 11 19.298z" />
        <path d="m16.5 14.5 5-5" />
        <path d="m16.5 9.5 5 5" />
      </>
    ),
    fill: (
      <>
        <path d="M11 4.702a.7.7 0 0 0-1.203-.498L6.413 7.587A1.4 1.4 0 0 1 5.416 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.7.7 0 0 0 11 19.298z" />
      </>
    ),
  },
  reset: {
    lucide: "rotate-ccw",
    stroke: (
      <>
        <path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" />
        <path d="M3 3v5h5" />
      </>
    ),
  },
  check: {
    lucide: "check",
    stroke: (
      <>
        <path d="M20 6 9 17l-5-5" />
      </>
    ),
  },
  plus: {
    lucide: "plus",
    stroke: (
      <>
        <path d="M5 12h14" />
        <path d="M12 5v14" />
      </>
    ),
  },
  link: {
    lucide: "link",
    stroke: (
      <>
        <path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71" />
        <path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71" />
      </>
    ),
  },
  paste: {
    lucide: "clipboard-paste",
    stroke: (
      <>
        <path d="M11 14h10" />
        <path d="M16 4h2a2 2 0 0 1 2 2v1.344" />
        <path d="m17 18 4-4-4-4" />
        <path d="M8 4H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 1.793-1.113" />
        <rect x="8" y="2" width="8" height="4" rx="1" />
      </>
    ),
    fill: (
      <>
        <rect x="8" y="2" width="8" height="4" rx="1" />
      </>
    ),
  },
  copy: {
    lucide: "copy",
    stroke: (
      <>
        <rect width="14" height="14" x="8" y="8" rx="2" ry="2" />
        <path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2" />
      </>
    ),
    fill: (
      <>
        <rect width="14" height="14" x="8" y="8" rx="2" ry="2" />
      </>
    ),
  },
  pencil: {
    lucide: "pencil",
    stroke: (
      <>
        <path d="M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497z" />
        <path d="m15 5 4 4" />
      </>
    ),
    fill: (
      <>
        <path d="M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497z" />
      </>
    ),
  },
  trash: {
    lucide: "trash-2",
    stroke: (
      <>
        <path d="M10 11v6" />
        <path d="M14 11v6" />
        <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6" />
        <path d="M3 6h18" />
        <path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
      </>
    ),
  },
  hash: {
    lucide: "hash",
    stroke: (
      <>
        <line x1="4" x2="20" y1="9" y2="9" />
        <line x1="4" x2="20" y1="15" y2="15" />
        <line x1="10" x2="8" y1="3" y2="21" />
        <line x1="16" x2="14" y1="3" y2="21" />
      </>
    ),
  },
} as const satisfies Record<string, IconShape>;
