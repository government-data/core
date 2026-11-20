/**
 * A source table's records (decision 0049, point 3), pure: what each way
 * of reading asks of the API, and why a page was refused, as the screen
 * names it.
 */

import { QualityError, type RecordsAsk } from "./api";

/** The first records, the last ones by a column, or every page in an order. */
export type Mode = "first" | "last" | "all";

/** What `mode` asks of `table`: page `page` (from 0); the last ones need `order`. */
export function askFor(table: string, mode: Mode, order: string, page: number): RecordsAsk {
  const by = order || null;
  switch (mode) {
    case "first":
      return { table, order: by, descending: false, page: 0 };
    case "last":
      return { table, order: by, descending: true, page: 0 };
    case "all":
      return { table, order: by, descending: false, page };
  }
}

/** Whether `mode` can be asked as chosen: the last ones need a column. */
export const ready = (mode: Mode, order: string) => mode !== "last" || order !== "";

/** Why the API refused a page: `table`, `order`, `no_order`, `too_far`, `forbidden` or `other`. */
export function refusal(e: unknown): string {
  if (!(e instanceof QualityError)) return "other";
  const error = (e.body as { error?: string } | undefined)?.error;
  if (e.status === 422 && error) return error;
  return e.status === 403 ? "forbidden" : "other";
}
