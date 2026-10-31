/** The longest name, as `MAX_NAME_CHARS` of `oc-proto`. */
export const MAX_NAME_CHARS = 32;

/** The same rule a camp applies (`clean_name` of `oc-proto`), or why not; `null` when fine. */
export function nameProblem(name: string): string | null {
  const trimmed = name.trim();
  if (trimmed === "") return "Введите имя";
  if ([...trimmed].length > MAX_NAME_CHARS) return `Не длиннее ${MAX_NAME_CHARS} символов`;
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f-\u009f]/.test(trimmed)) return "Без управляющих символов";
  return null;
}

/** Whether there is something to save: a fitting name that differs from the saved one. */
export function canSaveName(name: string, saved: string): boolean {
  return nameProblem(name) === null && name.trim() !== saved;
}
