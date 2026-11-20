import { createSignal } from "solid-js";

/** Where the fields chosen per dataset are kept in this browser. */
const STORE = "qd:test-columns";

function stored(): Record<string, string[]> {
  try {
    const raw = localStorage.getItem(STORE);
    return raw ? (JSON.parse(raw) as Record<string, string[]>) : {};
  } catch {
    return {};
  }
}

/**
 * The fields a rule's test shows besides the key and those the rule reads
 * (decision 0043, point 3), chosen per dataset and kept in this browser.
 */
const [chosen, setChosen] = createSignal(stored());

export const columnsOf = (dataset: string): string[] => chosen()[dataset] ?? [];

export const chooseColumns = (dataset: string, fields: string[]) => {
  const next = { ...chosen(), [dataset]: fields };
  setChosen(next);
  try {
    localStorage.setItem(STORE, JSON.stringify(next));
  } catch {
    // Kept for this page only.
  }
};
