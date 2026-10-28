import { ICONS, type IconShape } from "./icon-shapes.tsx";

/**
 * Lucide icons in the solid style (ADR-0027): closed parts filled, lines as Lucide
 * draws them, on its 24×24 grid with a 2px round stroke. Shapes: `icon-shapes.tsx`.
 */
export type IconName = keyof typeof ICONS;

/** Icons with their own "off" drawing in Lucide. */
const OFF = { mic: "micOff", headphones: "headphonesOff" } as const satisfies Partial<Record<IconName, IconName>>;

interface IconProps {
  name: IconName;
  size?: number;
  /** The crossed-out drawing (microphone, headphones). */
  off?: boolean;
  /** Accessible label; omit for decorative icons next to visible text. */
  label?: string;
  className?: string;
}

export function Icon({ name, size = 20, off = false, label, className }: IconProps) {
  const shown: IconName = off && name in OFF ? OFF[name as keyof typeof OFF] : name;
  const shape: IconShape = ICONS[shown];
  const a11y = label ? { role: "img", "aria-label": label } : { "aria-hidden": true };
  return (
    <svg
      className={className ? `icon ${className}` : "icon"}
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
      strokeLinejoin="round"
      {...a11y}
    >
      {shape.fill !== undefined && (
        <g fill="currentColor" stroke="none" fillRule="evenodd">
          {shape.fill}
        </g>
      )}
      {shape.stroke}
    </svg>
  );
}
