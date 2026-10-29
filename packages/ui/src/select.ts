// The pure part of `Select.tsx`: where the highlight goes on a key (the listbox pattern
// of WAI-ARIA), and type-to-find by the first letters.

/** One choice of a select. */
export interface SelectOption<T extends string = string> {
  readonly value: T;
  readonly label: string;
  /** Muted text on the right (an IP, a count). */
  readonly detail?: string;
}

/**
 * Where the highlight moves for `key` in a list of `count` options, from `current`
 * (-1: none yet). `null`: the key is not a move.
 */
export function moveHighlight(current: number, key: string, count: number): number | null {
  if (count === 0) return null;
  switch (key) {
    case "ArrowDown":
      return current < 0 ? 0 : Math.min(current + 1, count - 1);
    case "ArrowUp":
      return current < 0 ? count - 1 : Math.max(current - 1, 0);
    case "Home":
    case "PageUp":
      return 0;
    case "End":
    case "PageDown":
      return count - 1;
    default:
      return null;
  }
}

/**
 * The option a typed prefix leads to: the first whose label starts with it (any case),
 * looking after `current` first so repeating a letter walks through the matches.
 */
export function findByPrefix(options: readonly SelectOption[], prefix: string, current: number): number | null {
  const wanted = prefix.toLocaleLowerCase();
  if (wanted === "") return null;
  const n = options.length;
  // A single repeated letter steps to the next match; a longer prefix starts at the current.
  const step = wanted.length === 1 ? 1 : 0;
  for (let i = 0; i < n; i++) {
    const index = (Math.max(current, 0) + step + i) % n;
    if (options[index]!.label.toLocaleLowerCase().startsWith(wanted)) return index;
  }
  return null;
}
