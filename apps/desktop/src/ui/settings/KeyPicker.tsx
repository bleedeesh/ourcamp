import { useEffect, useState } from "react";

/**
 * Takes the next key or side mouse button as a choice; Esc changes nothing. Only while waiting
 * does it look at the keyboard. A press that does not qualify leaves it waiting.
 */
export function KeyPicker<T extends { readonly label: string }>({
  value,
  empty,
  fromKey,
  fromMouse,
  onPick,
}: {
  value: T | null;
  empty: string;
  fromKey: (e: KeyboardEvent) => T | null;
  fromMouse: (e: MouseEvent) => T | null;
  onPick: (key: T) => void;
}) {
  const [waiting, setWaiting] = useState(false);

  useEffect(() => {
    if (!waiting) return undefined;
    const pick = (key: T | null) => {
      if (key === null) return;
      setWaiting(false);
      onPick(key);
    };
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      if (e.code === "Escape") setWaiting(false);
      else pick(fromKey(e));
    };
    const onMouse = (e: MouseEvent) => {
      const key = fromMouse(e);
      if (key !== null) e.preventDefault();
      pick(key);
    };
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("mousedown", onMouse, true);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("mousedown", onMouse, true);
    };
  }, [waiting, fromKey, fromMouse, onPick]);

  return (
    <button type="button" className="wide-button talk-key" data-waiting={waiting || undefined} onClick={() => setWaiting(!waiting)}>
      {waiting ? "Нажмите… (Esc — отмена)" : (value?.label ?? empty)}
    </button>
  );
}
