/**
 * The work area's tabs (decision 0043, point 1): the rules file open, of
 * `main` or of a draft, and those with something typed and not saved, which
 * stay until saved or closed; and what was typed in each and not saved, kept while the app is open so a tab changed back to has it.
 * The tabs are kept in the browser's tab, the text only in memory.
 */

import { createSignal, untrack } from "solid-js";

/** A file open: of `main`, or of draft `draft`. */
export interface OpenFile {
  dataset: string;
  file: string;
  draft?: string;
}

const STORED = "qd.editor.tabs";

/** A tab's identity. */
export const keyOf = (f: OpenFile) => `${f.dataset}/${f.file}${f.draft ? `@${f.draft}` : ""}`;

/** Where a tab opens. */
export const pathOf = (f: OpenFile) =>
  `/quality/rules/${encodeURIComponent(f.dataset)}/${encodeURIComponent(f.file)}` +
  (f.draft ? `?draft=${encodeURIComponent(f.draft)}` : "");

/**
 * The tabs once `f` is opened: `f` in place of the others, which do not pile
 * up; only one with something typed and not saved stays, so it is not lost.
 */
export function opened(
  open: OpenFile[],
  f: OpenFile,
  unsaved: (o: OpenFile) => boolean = () => false,
): OpenFile[] {
  const kept = open.filter((o) => keyOf(o) === keyOf(f) || unsaved(o));
  return kept.some((o) => keyOf(o) === keyOf(f)) ? kept : [...kept, f];
}

/** `open` without `f`, and the tab to show after it: the one after it, else the one before. */
export function closed(open: OpenFile[], f: OpenFile): { open: OpenFile[]; next?: OpenFile } {
  const at = open.findIndex((o) => keyOf(o) === keyOf(f));
  if (at < 0) return { open };
  const rest = open.filter((_, i) => i !== at);
  return { open: rest, next: rest[at] ?? rest[at - 1] };
}

function read(): OpenFile[] {
  try {
    const kept = JSON.parse(sessionStorage.getItem(STORED) ?? "[]") as unknown;
    return Array.isArray(kept) ? (kept as OpenFile[]) : [];
  } catch {
    return [];
  }
}

const [tabs, setTabs] = createSignal<OpenFile[]>(read());
const UNSAVED = new Map<string, string>();
const [unsavedKeys, setUnsavedKeys] = createSignal<ReadonlySet<string>>(new Set());

function keep(open: OpenFile[]) {
  setTabs(open);
  try {
    sessionStorage.setItem(STORED, JSON.stringify(open));
  } catch {
    // Without storage, a reload forgets the tabs: nothing else does.
  }
}

const markUnsaved = () => setUnsavedKeys(new Set(UNSAVED.keys()));

/** The tabs, and what each keeps unsaved. */
export const openFiles = {
  tabs,
  /** Opens `f`; what calls it follows `f` only, not the tabs. */
  open: (f: OpenFile) => untrack(() => keep(opened(tabs(), f, (o) => UNSAVED.has(keyOf(o))))),
  /** Closes `f`, forgetting what it kept; the tab to show next, if any. */
  close: (f: OpenFile) => {
    const { open, next } = closed(tabs(), f);
    UNSAVED.delete(keyOf(f));
    markUnsaved();
    keep(open);
    return next;
  },
  /** What was typed in `f` and not saved, if anything. */
  unsaved: (f: OpenFile) => UNSAVED.get(keyOf(f)),
  /** Whether `f` has something unsaved: followed as it changes. */
  isUnsaved: (f: OpenFile) => unsavedKeys().has(keyOf(f)),
  /** Keeps `text` as typed in `f`; `undefined` once it is saved or restored. */
  type: (f: OpenFile, text: string | undefined) => {
    const was = UNSAVED.has(keyOf(f));
    if (text === undefined) UNSAVED.delete(keyOf(f));
    else UNSAVED.set(keyOf(f), text);
    if (was !== UNSAVED.has(keyOf(f))) markUnsaved();
  },
};
