/**
 * Puts text on the clipboard. The async Clipboard API can be refused (permissions,
 * focus, embedded webviews); the old selection-based copy still works in those cases.
 * Rejects with a message that contains the text itself, so the user can still copy it.
 */
export async function copyText(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
    return;
  } catch {
    // Fall through to the selection-based copy.
  }
  const area = document.createElement("textarea");
  area.value = text;
  area.setAttribute("readonly", "");
  area.style.position = "fixed";
  area.style.opacity = "0";
  document.body.append(area);
  area.select();
  // Deprecated, but the one copy path that does not depend on clipboard permissions.
  const copied = document.execCommand("copy");
  area.remove();
  if (!copied) throw new Error(`Не удалось скопировать — вот ссылка: ${text}`);
}
