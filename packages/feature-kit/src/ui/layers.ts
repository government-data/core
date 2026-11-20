/**
 * The dialogs open, the last opened on top. A dialog opened from another
 * (the reason to show values, from the console's) sits above it: Escape
 * closes only the top one, though every open dialog listens on `document`.
 */
const open: symbol[] = [];

/** A dialog that opened: whether it is on top, and its close. */
export interface Layer {
  top: () => boolean;
  closed: () => void;
}

/** Puts a dialog on top of those open. */
export function opened(): Layer {
  const me = Symbol("dialog");
  open.push(me);
  return {
    top: () => open.at(-1) === me,
    closed: () => {
      const at = open.indexOf(me);
      if (at >= 0) open.splice(at, 1);
    },
  };
}

/**
 * Whether `layer` takes this Escape: the top dialog does, once (it marks the
 * event taken, so no other closes, whichever listener runs first).
 */
export function takesEscape(
  layer: Layer,
  e: Pick<KeyboardEvent, "key" | "defaultPrevented" | "preventDefault">,
): boolean {
  if (e.key !== "Escape" || e.defaultPrevented || !layer.top()) return false;
  e.preventDefault();
  return true;
}
