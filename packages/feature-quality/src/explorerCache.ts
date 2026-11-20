/**
 * What the explorer asks of the API more than once, asked once while the
 * page is open (decision 0043): each dataset's rules, the schedules, the
 * drafts and who is looking. A refusal is not kept, so it is asked again.
 */

import { createSignal } from "solid-js";
import { draftsApi, me, rulesApi, schedulesApi, type Me } from "./api";
import { columnOf, parseBranch } from "./drafts";

const KEPT = new Map<string, Promise<unknown>>();

function once<T>(key: string, load: () => Promise<T>): Promise<T> {
  const known = (KEPT.get(key) as Promise<T> | undefined) ?? load();
  KEPT.set(key, known);
  known.catch(() => KEPT.delete(key));
  return known;
}

export const rulesOf = (dataset: string) => once(`rules/${dataset}`, () => rulesApi.read(dataset));

/**
 * Bumped when a dataset's rules reach `main` from the explorer (_Deploy_,
 * decision 0051): its rules, and the columns' histories, are asked again.
 */
const [ruled, setRuled] = createSignal(1);
export { ruled };
export const rulesChanged = (dataset: string) => {
  KEPT.delete(`rules/${dataset}`);
  setRuled((n) => n + 1);
};
export const allSchedules = () => once("schedules", schedulesApi.list);
export const allDrafts = () => once("drafts", draftsApi.list);
export const who = () => once("me", me);

/** Who is signed in, once known: a menu made in a click does not wait for it. */
let known: Me | undefined;
void who()
  .then((m) => (known = m))
  .catch(() => undefined);
export const knownMe = () => known;

/** Bumped when a schedule is saved from the explorer: the schedules are asked again. */
const [scheduled, setScheduled] = createSignal(1);
export { scheduled };
export const schedulesChanged = () => {
  KEPT.delete("schedules");
  setScheduled((n) => n + 1);
};

/**
 * Bumped when `main`'s mappings change from the explorer (_Map the whole
 * table_, decision 0047): the datasets are asked again, so the new fields
 * show.
 */
const [mappings, setMappings] = createSignal(1);
export { mappings };
export const mappingsChanged = () => setMappings((n) => n + 1);

/** Bumped when a listing changed here (a table deleted, decision 0055): the listings asked again. */
const [listings, setListings] = createSignal(1);
export { listings };
export const listingsChanged = () => setListings((n) => n + 1);

/** Where a dataset's node is in the tree, under the lake. */
export const datasetPlace = (dataset: string) => `lake/+datasets/${dataset}`;

/**
 * The one table or dataset marked in the tree, by its place: the last
 * opened by a click on its row, or linked to by _→ dataset_.
 */
const [picked, setPicked] = createSignal<string>();
export { picked };

/** A click on the row at `place`: opened, it is marked; closed, unmarked if it was. */
export const pick = (place: string, opened: boolean) => {
  if (opened) setPicked(place);
  else if (picked() === place) setPicked(undefined);
};

/**
 * The dataset a source's table asked to see (decision 0049, _→ dataset_):
 * the lake opened, that dataset's node marked and scrolled to.
 */
const [revealed, setRevealed] = createSignal<{ name: string; at: number }>();
export { revealed };
export const reveal = (name: string) => {
  setPicked(datasetPlace(name));
  setRevealed({ name, at: Date.now() });
};

/** A dataset's open draft, with its column's file (decision 0050), if it is one's. */
export interface OpenDraft {
  id: string;
  author: string;
  at: string;
  file?: string;
}

/** Bumped when a draft is approved or discarded from the explorer: the drafts are asked again. */
const [drafted, setDrafted] = createSignal(1);
export { drafted };
export const draftsChanged = () => {
  for (const key of [...KEPT.keys()]) if (key.startsWith("drafts")) KEPT.delete(key);
  setDrafted((n) => n + 1);
};

/** `dataset`'s open drafts, newest first, each with the column it changes. */
export const draftsOf = (dataset: string) =>
  once(`drafts/${dataset}`, async (): Promise<OpenDraft[]> => {
    const mine = (await allDrafts()).flatMap((d) => {
      const at = parseBranch(d.branch);
      return at?.dataset === dataset ? [{ id: at.id, author: d.author, at: d.at }] : [];
    });
    const files = await Promise.all(mine.map((d) => draftsApi.changes(dataset, d.id)));
    return mine
      .map((d, i) => ({ ...d, file: columnOf(files[i]?.files ?? []) }))
      .sort((a, b) => b.at.localeCompare(a.at));
  });
