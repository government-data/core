/**
 * The drafts' pure parts (decisions 0021, 0038 and 0050): which dataset
 * and proposal a branch is, which column a draft is, whether a reviewed
 * draft may be approved, and what a refused approval says.
 */

import { formatDateTime } from "@knime-orchestador/i18n";
import type { DraftOut, FileChange } from "./api";

/** A draft's dataset and id, from its branch `proposal/<dataset>/<id>`. */
export function parseBranch(branch: string): { dataset: string; id: string } | undefined {
  const [prefix, dataset, id, ...rest] = branch.split("/");
  if (prefix !== "proposal" || !dataset || !id || rest.length > 0) return undefined;
  return { dataset, id };
}

/** Its lint errors, all files together. */
export function errorCount(draft: DraftOut): number {
  return draft.rules.reduce((n, file) => n + file.errors.length, 0);
}

/** Whether `main` would take it: its mapping reads and no file has errors. */
export function approvable(draft: DraftOut): boolean {
  return draft.mapping_reads && errorCount(draft) === 0;
}

/** The text key of a refused approval, by its status and the API's code. */
export function approvalRefusal(status: number, body: unknown): string {
  const code = (body as { error?: string } | undefined)?.error;
  if (status === 409 && (code === "stale" || code === "conflicts")) return code;
  if (status === 422) return code === "unknown_source" ? "unknownSource" : "lint";
  if (status === 403) return "forbidden";
  if (status === 404) return "notFound";
  return "other";
}

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** An author `Name <email>` as the list shows it: the name, an account id shortened. */
export function authorName(author: string): string {
  const name = author.replace(/\s*<[^>]*>\s*$/, "").trim() || author;
  return UUID.test(name) ? `${name.slice(0, 8)}…` : name;
}

/**
 * A draft's column (decision 0050): the one rules file it changes, if that
 * is all it changes; `undefined` for a draft of the dataset (its mapping,
 * or several files).
 */
export function columnOf(files: FileChange[]): string | undefined {
  const [one, ...rest] = files;
  return one && rest.length === 0 && one.file !== "mapping" ? one.file : undefined;
}

/** A Dolt date as the person reads it, or as it came if it is not one. */
export function doltDate(at: string): string {
  const date = new Date(at.includes("T") ? at : at.replace(" ", "T") + "Z");
  return Number.isNaN(date.getTime()) ? at : formatDateTime(date.toISOString());
}
