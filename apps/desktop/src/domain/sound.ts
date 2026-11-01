// The choices of a sound device and of how the microphone turns on, in the settings window
// (ADR-0066, ADR-0069).

import type { TalkKey } from "../engine/sound-api.ts";

/** The range of the threshold, dBFS; `MIN_THRESHOLD_DB` and `MAX_THRESHOLD_DB` in `oc-audio`. */
export const MIN_THRESHOLD_DB = -70;
export const MAX_THRESHOLD_DB = -10;

/** Where `db` falls on the bar and the slider above it, percent. */
export function meterPercent(db: number): number {
  const percent = ((db - MIN_THRESHOLD_DB) / (MAX_THRESHOLD_DB - MIN_THRESHOLD_DB)) * 100;
  return Math.min(100, Math.max(0, percent));
}

const KEY_NAMES: Readonly<Record<string, string>> = {
  Space: "Пробел",
  Tab: "Tab",
  CapsLock: "Caps Lock",
  Enter: "Enter",
  Backspace: "Backspace",
  ShiftLeft: "Shift",
  ShiftRight: "Shift",
  ControlLeft: "Ctrl",
  ControlRight: "Ctrl",
  AltLeft: "Alt",
  AltRight: "Alt",
  Backquote: "`",
  Minus: "-",
  Equal: "=",
  BracketLeft: "[",
  BracketRight: "]",
  Backslash: "\\",
  Semicolon: ";",
  Quote: "'",
  Comma: ",",
  Period: ".",
  Slash: "/",
};

/**
 * The talk key for a key press, by the page's `code` (the key's place, so the label does not
 * change with the layout) and `keyCode` (on Windows, the virtual-key code the engine reads).
 * The Windows key opens the system's menu, so it does not qualify.
 */
export function talkKeyFromPress(code: string, keyCode: number): TalkKey | null {
  if (keyCode === 0 || code === "MetaLeft" || code === "MetaRight") return null;
  const label =
    KEY_NAMES[code] ?? /^(?:Key|Digit)(.)$/.exec(code)?.[1] ?? code.replace(/^Numpad/, "Num ").replace(/^Arrow/, "Стрелка ");
  return { code: keyCode, label };
}

/** The talk key for a mouse button; of them, only the side ones are free for this. */
export function talkKeyFromMouse(button: number): TalkKey | null {
  // `button` counts from 0; these are the virtual-key codes VK_XBUTTON1 and VK_XBUTTON2.
  if (button === 3) return { code: 0x05, label: "Кнопка мыши 4" };
  if (button === 4) return { code: 0x06, label: "Кнопка мыши 5" };
  return null;
}

/** The threshold as the label says it. */
export function thresholdLabel(db: number): string {
  return `${String(Math.round(db)).replace("-", "−")} дБ`;
}

/** A device choice as the list has it: the name, or this for "as in the system". */
export const AS_IN_SYSTEM = "";

export interface DeviceOption {
  readonly value: string;
  readonly label: string;
  readonly detail?: string;
}

/**
 * "As in the system" first, naming what that is now, then every device. A chosen device that
 * is not connected stays on the list: it is what the user wants back, and the app returns to
 * it when it is plugged in again.
 */
export function deviceOptions(
  devices: readonly string[],
  systemDevice: string | null,
  chosen: string | null,
): readonly DeviceOption[] {
  const options: DeviceOption[] = [
    systemDevice === null
      ? { value: AS_IN_SYSTEM, label: "Как в системе", detail: "нет устройства" }
      : { value: AS_IN_SYSTEM, label: `Как в системе: ${systemDevice}` },
    ...devices.map((name) => ({ value: name, label: name })),
  ];
  if (chosen !== null && !devices.includes(chosen)) {
    options.push({ value: chosen, label: chosen, detail: "не подключено" });
  }
  return options;
}

/** The list's value for a choice. */
export function deviceValue(chosen: string | null): string {
  return chosen ?? AS_IN_SYSTEM;
}

/** The choice for a list's value. */
export function deviceChoice(value: string): string | null {
  return value === AS_IN_SYSTEM ? null : value;
}
