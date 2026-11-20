/**
 * The rules screen's pure parts (decision 0019, _Rules of a dataset_):
 * a dataset's rules from all its files in one list, the values each filter
 * offers, the rules a filter keeps, and their order.
 */

import type { FindingOut, RulesOut, RuleView } from "./api";

/** A rule with the file it is in. */
export type Row = RuleView & { file: string };

/** What the person filters by; empty means any. */
export interface Filter {
  text: string;
  pattern: string;
  dimension: string;
  severity: string;
}

export const NO_FILTER: Filter = { text: "", pattern: "", dimension: "", severity: "" };

/** Every rule of every file, in file order. */
export function rows(out: RulesOut): Row[] {
  return out.files.flatMap((f) => f.rules.map((r) => ({ ...r, file: f.file })));
}

/** The files with lint errors, and their errors. */
export function broken(out: RulesOut): { file: string; errors: FindingOut[] }[] {
  return out.files
    .map((f) => ({ file: f.file, errors: f.findings.filter((x) => x.severity === "error") }))
    .filter((f) => f.errors.length > 0);
}

/** The values one column takes, sorted, each once. */
export function options(all: Row[], key: "pattern" | "dimension" | "severity"): string[] {
  return [...new Set(all.map((r) => r[key]))].sort((a, b) => a.localeCompare(b));
}

/** The rules `filter` keeps: the text in the id, field or what fails, any case. */
export function keep(all: Row[], filter: Filter): Row[] {
  const text = filter.text.trim().toLocaleLowerCase();
  return all.filter(
    (r) =>
      (!filter.pattern || r.pattern === filter.pattern) &&
      (!filter.dimension || r.dimension === filter.dimension) &&
      (!filter.severity || r.severity === filter.severity) &&
      (!text || [r.id, r.field ?? "", r.fails].some((v) => v.toLocaleLowerCase().includes(text))),
  );
}

/** A column the table sorts by, and which way. */
export type SortKey = "id" | "field" | "pattern" | "dimension" | "severity" | "fails" | "file";
export interface Sort {
  key: SortKey;
  descending: boolean;
}

/** By id, as a person counts: `V-PN-2` before `V-PN-10`. */
export const BY_ID: Sort = { key: "id", descending: false };

const natural = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });

/** The rules in `sort`'s order, its ties by id; a rule of no field last by field. */
export function sorted(all: Row[], sort: Sort): Row[] {
  const value = (r: Row) => (sort.key === "field" ? (r.field ?? "\uffff") : r[sort.key]);
  const way = sort.descending ? -1 : 1;
  return [...all].sort(
    (a, b) => way * natural.compare(value(a), value(b)) || natural.compare(a.id, b.id),
  );
}
