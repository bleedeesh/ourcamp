// The choices of a sound device in the settings window (ADR-0066).

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
