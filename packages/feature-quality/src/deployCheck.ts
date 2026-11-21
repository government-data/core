/**
 * What a deploy, a restore or an undo would put on `main` (decision 0051),
 * read by the linter, pure: the findings that block it (errors) and those
 * to see first (warnings), and which rules it adds, removes or changes
 * against `main`'s file.
 */

import type { CheckOut, FindingOut } from "./api";

export interface DeployReview {
  errors: FindingOut[];
  warnings: FindingOut[];
  added: string[];
  removed: string[];
  changed: string[];
}

type Rule = CheckOut["rules"][number];

/** A rule as it reads, to tell whether it changed: its compiled SQL left out. */
const reading = (r: Rule) => JSON.stringify({ ...r, duckdb: undefined });

/** `after` (the file to deploy) against `before` (`main`'s, if it has one). */
export function review(before: CheckOut | undefined, after: CheckOut): DeployReview {
  const was = new Map((before?.rules ?? []).map((r) => [r.id, reading(r)]));
  const now = new Map(after.rules.map((r) => [r.id, reading(r)]));
  return {
    errors: after.findings.filter((f) => f.severity === "error"),
    warnings: after.findings.filter((f) => f.severity !== "error"),
    added: [...now.keys()].filter((id) => !was.has(id)),
    removed: [...was.keys()].filter((id) => !now.has(id)),
    changed: [...now.keys()].filter((id) => was.has(id) && was.get(id) !== now.get(id)),
  };
}
