/**
 * What the sources' page computes (decision 0036), pure: a form's values as
 * the API's body, split by the driver into fields and secrets (a secret left
 * empty is left out, so the stored one is kept), and the API's refusals as
 * the texts a person reads.
 */

import {
  QualityError,
  type Driver,
  type ListingView,
  type Refusal,
  type SourceBody,
  type SourceTable,
} from "./api";
import { t } from "./i18n";

/** What a mapping and a Trino catalog may call a source. */
const NAME = /^[a-z][a-z0-9_]{0,62}$/;

export function validName(name: string): boolean {
  return NAME.test(name);
}

/** The body for `driver` from the form's `values`, by field key. */
export function sourceBody(driver: Driver, values: Record<string, string>): SourceBody {
  const fields: Record<string, string> = {};
  const secrets: Record<string, string> = {};
  for (const f of driver.fields) {
    const value = (values[f.key] ?? "").trim();
    if (!value) continue;
    if (f.kind === "secret") secrets[f.key] = values[f.key] ?? "";
    else fields[f.key] = value;
  }
  return { driver: driver.id, fields, secrets };
}

/** The required fields still empty; a secret already set does not count. */
export function missing(driver: Driver, values: Record<string, string>, set: string[]): string[] {
  return driver.fields
    .filter((f) => f.required && !(values[f.key] ?? "").trim())
    .filter((f) => !(f.kind === "secret" && set.includes(f.key)))
    .map((f) => f.key);
}

/** A field's label in the person's language. */
export function fieldLabel(key: string): string {
  const known = ["host", "port", "database", "service", "user", "password", "options"];
  return known.includes(key) ? t(`quality:sources.field.${key}`) : key;
}

/** A refusal of the API as a text. */
export function refusalText(r: Refusal): string {
  const field = r.field ? fieldLabel(r.field) : "";
  const known = [
    "name",
    "unknown_driver",
    "unknown_field",
    "wrong_place",
    "missing",
    "port",
    "value",
  ];
  const code = known.includes(r.code) ? r.code : "other";
  return t(`quality:sources.refusal.${code}`, { field });
}

/** A listed table as a map names it (decision 0042). */
export function tableRef(table: SourceTable): string {
  return `${table.schema}.${table.name}`;
}

/** The tables whose `schema.name` holds every word of `query`, any case. */
export function filterTables(tables: SourceTable[], query: string): SourceTable[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return tables;
  return tables.filter((table) => {
    const ref = tableRef(table).toLowerCase();
    return words.every((w) => ref.includes(w));
  });
}

/** Whether a listing is on its way: queued, running or retrying. */
export const onItsWay = (l: ListingView | undefined) =>
  l?.state === "listing" || l?.state === "retrying";

/** Why a deletion was refused, in the person's words. */
export function deleteRefusal(err: unknown): string {
  if (!(err instanceof QualityError)) return t("quality:common.saveError");
  if (err.status === 409) {
    const datasets = (err.body as { datasets?: string[] } | undefined)?.datasets ?? [];
    return t("quality:sources.inUse", { datasets: datasets.join(", ") });
  }
  if (err.status === 403) return t("quality:common.forbidden");
  if (err.status === 502) return t("quality:sources.trinoDelete");
  if (err.status === 503) return t("quality:common.catalogUnavailable");
  return t("quality:common.saveError");
}

/** Why a source was not saved, when it is no refusal of its fields. */
export function saveFailure(err: unknown): string {
  if (err instanceof QualityError && err.status === 502) return t("quality:sources.trinoSave");
  return t("quality:common.saveError");
}
