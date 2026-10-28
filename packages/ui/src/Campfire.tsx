/**
 * The OurCamp campfire mark. Same geometry as the brand sheet (docs/design.md).
 * `simple` drops the outline, log ends and sparks for sizes ≤ 24 px where they turn to
 * mush; `dim` renders an extinguished (empty channel) fire; `server` adds the server
 * badge of the server window's app icon (ADR-0032).
 */
interface CampfireProps {
  size: number;
  simple?: boolean;
  dim?: boolean;
  server?: boolean;
  label?: string;
}

// Night palette (ADR-0024): violet flame, dusk logs.
const INK = "#1B1633";

export function Campfire({ size, simple = false, dim = false, server = false, label }: CampfireProps) {
  const outline = simple ? {} : { stroke: INK, strokeWidth: 1.6, strokeLinejoin: "round" as const };
  const a11y = label ? { role: "img", "aria-label": label } : { "aria-hidden": true };
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 64 64"
      className={dim ? "campfire campfire--dim" : "campfire"}
      {...a11y}
    >
      <rect x="10" y="46" width="44" height="9" rx="4.5" fill="#3E3470" transform="rotate(-16 32 50.5)" {...outline} />
      <rect x="10" y="46" width="44" height="9" rx="4.5" fill="#3E3470" transform="rotate(16 32 50.5)" {...outline} />
      {!simple && (
        <>
          <circle cx="15.4" cy="55.2" r="4.3" fill="#6E62B8" {...outline} />
          <circle cx="15.4" cy="55.2" r="1.7" fill="#3E3470" />
          <circle cx="48.6" cy="55.2" r="4.3" fill="#6E62B8" {...outline} />
          <circle cx="48.6" cy="55.2" r="1.7" fill="#3E3470" />
        </>
      )}
      <path
        d="M32 5C36.5 13 46.5 20 46.5 33C46.5 42 40 48.5 32 48.5C24 48.5 17.5 42 17.5 33C17.5 26 21.5 21.5 25 16.5C26 22.5 28 25 30.2 25C29 18 30 11 32 5Z"
        fill="#8B7CF6"
        {...outline}
      />
      <path
        d="M32 18C35 24 40 28 40 36C40 42 36.5 46 32 46C27.5 46 24 42 24 36C24 31 27 28 29 25C30 29 31 30 32.5 30C31.5 26 31 22 32 18Z"
        fill="#B3A8FF"
      />
      <path d="M32 30C34 33 36 35.5 36 39C36 42.5 34.2 45 32 45C29.8 45 28 42.5 28 39C28 35.5 30 33 32 30Z" fill="#ECE8FF" />
      {!simple && (
        <>
          <circle cx="48" cy="11" r="1.7" fill="#C4BBFF" />
          <circle cx="15.5" cy="14" r="1.2" fill="#C4BBFF" />
          <circle cx="51.5" cy="20" r="1" fill="#C4BBFF" />
        </>
      )}
      {server && (
        // Two rack units with lit lamps, as in apps/server/app-icon.svg.
        <>
          <rect x="39" y="40" width="24" height="23" rx="6" fill="#8B7CF6" stroke={INK} strokeWidth="1.6" />
          <rect x="43" y="44.5" width="16" height="6" rx="2" fill={INK} />
          <rect x="43" y="52.5" width="16" height="6" rx="2" fill={INK} />
          {!simple && (
            <>
              <circle cx="46.3" cy="47.5" r="1.3" fill="#3DDC84" />
              <circle cx="46.3" cy="55.5" r="1.3" fill="#3DDC84" />
            </>
          )}
        </>
      )}
    </svg>
  );
}
