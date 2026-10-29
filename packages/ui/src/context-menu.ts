// The webview's own right-click menu (Back, Reload, Inspect…) is a browser's, not ours
// (ADR-0034): the apps open their own menus on people, and elsewhere a right click does
// nothing — except where cut, copy and paste are what one wants.

/** Where the webview's own menu stays: fields one types in. */
const EDITABLE = 'input, textarea, [contenteditable=""], [contenteditable="true"]';

/** Just what is needed of an event target. */
interface Target {
  closest(selector: string): unknown;
}

/** Whether a right click on `target`, with `selected` text, keeps the webview's own menu. */
export function nativeMenuWanted(target: Target | null, selected: string): boolean {
  return selected.trim() !== "" || (target !== null && target.closest(EDITABLE) !== null);
}

/** Turns the webview's own menu off for the document, but for fields and selections. */
export function suppressNativeContextMenu(): () => void {
  const onMenu = (e: MouseEvent) => {
    // An app menu that opened already called preventDefault: nothing to do.
    if (e.defaultPrevented) return;
    const target = e.target instanceof Element ? e.target : null;
    if (!nativeMenuWanted(target, window.getSelection()?.toString() ?? "")) e.preventDefault();
  };
  document.addEventListener("contextmenu", onMenu);
  return () => document.removeEventListener("contextmenu", onMenu);
}
