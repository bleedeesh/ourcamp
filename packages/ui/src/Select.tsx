import { useEffect, useId, useLayoutEffect, useRef, useState, type KeyboardEvent } from "react";

import { Icon } from "./Icon.tsx";
import { findByPrefix, moveHighlight, type SelectOption } from "./select.ts";

export type { SelectOption } from "./select.ts";

/** How long typed letters add up into one prefix. */
const TYPEAHEAD_MS = 600;
/** Gap between the field and its list, and from the window's edge. */
const GAP = 4;
const MARGIN = 8;

interface SelectProps<T extends string> {
  value: T;
  options: readonly SelectOption<T>[];
  onChange: (value: T) => void;
  /** Called as the list opens (to look for fresh options). */
  onOpen?: () => void;
  /** The field's name for screen readers; or point at a visible label with `labelledBy`. */
  label?: string;
  labelledBy?: string;
  id?: string;
  /** `compact` for rows (a member's role), `regular` for forms. */
  size?: "regular" | "compact";
}

/**
 * The app's own select (the "select-only combobox" of WAI-ARIA): a field showing the
 * choice, and a list in the app's style with a muted detail on the right. Keyboard:
 * arrows, Home/End, Enter/Space, Escape, Tab, and type-to-find. The webview's own
 * <select> draws a system list that matches nothing around it.
 */
export function Select<T extends string>({
  value,
  options,
  onChange,
  onOpen,
  label,
  labelledBy,
  id,
  size = "regular",
}: SelectProps<T>) {
  const own = useId();
  const listId = `${id ?? own}-list`;
  const optionId = (i: number) => `${id ?? own}-option-${i}`;
  const trigger = useRef<HTMLButtonElement>(null);
  const list = useRef<HTMLUListElement>(null);
  const [open, setOpen] = useState(false);
  const [highlight, setHighlight] = useState(-1);
  const [place, setPlace] = useState<{ left: number; width: number; top?: number; bottom?: number } | null>(null);
  const typed = useRef({ text: "", at: 0 });
  const selected = options.findIndex((o) => o.value === value);
  const current = options[selected];

  const show = () => {
    onOpen?.();
    setHighlight(selected);
    setOpen(true);
  };
  const hide = (refocus = true) => {
    setOpen(false);
    setPlace(null);
    if (refocus) trigger.current?.focus();
  };
  const choose = (index: number) => {
    const option = options[index];
    hide();
    if (option && option.value !== value) onChange(option.value);
  };

  // Below the field, or above it when the window ends first; fixed, so no scrolling
  // card clips it (it closes on scroll instead of drifting).
  useLayoutEffect(() => {
    if (!open || !trigger.current || !list.current) return;
    const field = trigger.current.getBoundingClientRect();
    const height = list.current.offsetHeight;
    const below = window.innerHeight - field.bottom - GAP - MARGIN;
    const fitsBelow = height <= below || below >= field.top - GAP - MARGIN;
    setPlace({
      left: field.left,
      width: field.width,
      ...(fitsBelow ? { top: field.bottom + GAP } : { bottom: window.innerHeight - field.top + GAP }),
    });
  }, [open, options.length]);

  useEffect(() => {
    if (!open) return;
    const outside = (e: PointerEvent) => {
      const target = e.target as Node;
      if (!trigger.current?.contains(target) && !list.current?.contains(target)) hide(false);
    };
    const away = (e: Event) => {
      if (!list.current?.contains(e.target as Node)) hide(false);
    };
    document.addEventListener("pointerdown", outside);
    window.addEventListener("scroll", away, true);
    window.addEventListener("resize", away);
    return () => {
      document.removeEventListener("pointerdown", outside);
      window.removeEventListener("scroll", away, true);
      window.removeEventListener("resize", away);
    };
  });

  // The highlighted option stays in view as the arrows move it.
  useEffect(() => {
    if (open && highlight >= 0) document.getElementById(optionId(highlight))?.scrollIntoView({ block: "nearest" });
  });

  const onKey = (e: KeyboardEvent<HTMLButtonElement>) => {
    const moved = moveHighlight(highlight, e.key, options.length);
    if (!open) {
      if (["ArrowDown", "ArrowUp", "Enter", " "].includes(e.key)) {
        e.preventDefault();
        show();
      }
      return;
    }
    if (moved !== null) {
      e.preventDefault();
      setHighlight(moved);
    } else if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      if (highlight >= 0) choose(highlight);
    } else if (e.key === "Escape") {
      e.preventDefault();
      hide();
    } else if (e.key === "Tab") {
      hide(false);
    } else if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) {
      const now = Date.now();
      const text = (now - typed.current.at < TYPEAHEAD_MS ? typed.current.text : "") + e.key;
      typed.current = { text, at: now };
      const found = findByPrefix(options, text, highlight);
      if (found !== null) setHighlight(found);
    }
  };

  return (
    <div className="select" data-size={size}>
      <button
        ref={trigger}
        id={id}
        type="button"
        role="combobox"
        className="select__field"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={listId}
        aria-activedescendant={open && highlight >= 0 ? optionId(highlight) : undefined}
        aria-label={label}
        aria-labelledby={labelledBy}
        onClick={() => (open ? hide() : show())}
        onKeyDown={onKey}
      >
        <span className="select__value">{current?.label ?? ""}</span>
        {current?.detail && <span className="select__detail">{current.detail}</span>}
        <Icon name="chevron" size={16} className="select__chevron" />
      </button>
      {open && (
        <ul
          ref={list}
          id={listId}
          role="listbox"
          className="select__list"
          aria-label={label}
          aria-labelledby={labelledBy}
          style={place ?? { visibility: "hidden" }}
        >
          {options.map((option, i) => (
            <li
              key={option.value}
              id={optionId(i)}
              role="option"
              className="select__option"
              aria-selected={i === selected}
              data-highlighted={i === highlight || undefined}
              onPointerMove={() => setHighlight(i)}
              // Not a click: the field must not lose focus to the list before choosing.
              onPointerDown={(e) => e.preventDefault()}
              onClick={() => choose(i)}
            >
              <span className="select__value">{option.label}</span>
              {option.detail && <span className="select__detail">{option.detail}</span>}
              {i === selected && <Icon name="check" size={16} className="select__check" />}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
