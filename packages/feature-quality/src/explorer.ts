/**
 * The explorer's tree (decisions 0043 and 0049), pure: every source, the
 * lake among them, normalised to source → schema → table → column, and each
 * dataset a node of the lake with its fields and the rules that read them,
 * from the listings (0042's shape), the datasets' `from` and fields, and
 * the fields each rule reads. Names compare in any case, as catalogs
 * fold them differently.
 */

import type { DatasetOut, ListingView, RulesOut, SourceColumn, SourceTable } from "./api";
import { validName } from "./sources";

/** What a table or column is: read by a mapping, read by none, or read and no longer listed. */
export type Mark = "read" | "unread" | "gone";

export interface TableNode {
  schema: string;
  name: string;
  /** As the last listing has them; none for a table gone. */
  columns: SourceColumn[];
  /** The datasets that read it. */
  datasets: string[];
  mark: Mark;
}

export interface SchemaNode {
  name: string;
  tables: TableNode[];
}

/** A rule as the tree shows it: by its file, and the other fields it reads. */
export interface RuleLeaf {
  id: string;
  file: string;
  also: string[];
}

export interface ColumnNode {
  name: string;
  /** None for a column gone. */
  dataType?: string;
  /** The mapping's fields that read it. */
  fields: string[];
  rules: RuleLeaf[];
  mark: Mark;
  /** A field reads it and no rule checks that field. */
  gap: boolean;
}

/** A derived field, the fields it comes from, and its rules. */
export interface DerivedNode {
  field: string;
  from: string[];
  rules: RuleLeaf[];
}

const same = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();

/** Tables grouped by schema, both in name order. */
function bySchema(tables: TableNode[]): SchemaNode[] {
  const schemas = new Map<string, TableNode[]>();
  for (const table of tables)
    schemas.set(table.schema, [...(schemas.get(table.schema) ?? []), table]);
  return [...schemas]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([name, tables]) => ({
      name,
      tables: tables.sort((a, b) => a.name.localeCompare(b.name)),
    }));
}

/** Whether `dataset` reads `table` of `source`; a `from` without a schema reads it in any. */
function reads(dataset: DatasetOut, source: string, table: SourceTable): boolean {
  const from = dataset.from;
  if (!from || from.source !== source) return false;
  return same(from.table, table.name) && (!from.schema || same(from.schema, table.schema));
}

/**
 * The database a source's server holds, the level between the server and
 * its schemas: its `database`, or Oracle's `service`.
 */
export function databaseOf(fields: Record<string, string>): string | undefined {
  return fields.database || fields.service || undefined;
}

/**
 * A source's schemas and tables: each table with the datasets that read it,
 * and the tables a dataset reads that the listing does not have, gone. Until
 * the source is listed nothing is gone.
 */
export function sourceSchemas(
  source: string,
  listing: ListingView,
  datasets: DatasetOut[],
): SchemaNode[] {
  const tables: TableNode[] = listing.tables.map((table) => {
    const by = datasets.filter((d) => reads(d, source, table)).map((d) => d.name);
    return { ...table, datasets: by, mark: by.length ? "read" : "unread" };
  });
  if (listing.state !== "none" && !listing.truncated) {
    for (const d of datasets) {
      const from = d.from;
      if (!from || from.source !== source || listing.tables.some((t) => reads(d, source, t)))
        continue;
      tables.push({
        schema: from.schema ?? "",
        name: from.table,
        columns: [],
        datasets: [d.name],
        mark: "gone",
      });
    }
  }
  return bySchema(tables);
}

/** The table a dataset is started from, and the dataset's name. */
export interface NewDataset {
  dataset: string;
  table: { source?: string; table: string };
}

/**
 * The dataset a table no dataset reads would start (decision 0043, _Add to
 * the mapping_): in the lake, the one its `<dataset>_bronze` namespace
 * names; in a source, one named as its table, in lower case.
 */
export function newDataset(source: string | undefined, table: TableNode): NewDataset | undefined {
  if (!source) {
    const dataset = table.schema.endsWith("_bronze") ? table.schema.slice(0, -7) : "";
    return validName(dataset) ? { dataset, table: { table: table.name } } : undefined;
  }
  const dataset = table.name.toLowerCase().replace(/[^a-z0-9]+/g, "_");
  return validName(dataset)
    ? { dataset, table: { source, table: `${table.schema}.${table.name}` } }
    : undefined;
}

/** A dataset's namespace in the lake. */
const bronzeOf = (dataset: string) => `${dataset}_bronze`;

/** Whether `table` of the lake is `dataset`'s bronze table. */
const writes = (dataset: DatasetOut, table: SourceTable) =>
  !!dataset.bronze &&
  same(table.schema, bronzeOf(dataset.name)) &&
  same(table.name, dataset.bronze);

/** The lake's namespaces and tables, each with the datasets whose bronze table it is. */
export function lakeTables(listing: ListingView, datasets: DatasetOut[]): SchemaNode[] {
  const tables: TableNode[] = listing.tables.map((table) => {
    const by = datasets.filter((d) => writes(d, table)).map((d) => d.name);
    return { ...table, datasets: by, mark: by.length ? "read" : "unread" };
  });
  return bySchema(tables);
}

/**
 * The lake's tables the explorer lists, to map (decision 0049): those no
 * dataset reads; a dataset's bronze table is its node, not a second entry.
 */
export function lakeSchemas(listing: ListingView, datasets: DatasetOut[]): SchemaNode[] {
  return lakeTables(listing, datasets)
    .map((s) => ({ ...s, tables: s.tables.filter((t) => t.mark === "unread") }))
    .filter((s) => s.tables.length > 0);
}

/**
 * The table a dataset's node shows its fields against (decision 0049): its
 * source's table as `listing` has it, or its bronze table in the lake's;
 * without either (not listed, or not to be seen), its fields' columns with
 * no type; gone if the listing is there and lacks it.
 */
export function datasetTable(dataset: DatasetOut, listing?: ListingView): TableNode {
  const from = dataset.from;
  const found = listing?.tables.find((t) =>
    from ? reads(dataset, from.source, t) : writes(dataset, t),
  );
  if (found) return { ...found, datasets: [dataset.name], mark: "read" };
  const listed = !!listing && listing.state !== "none" && !listing.truncated;
  return {
    schema: from ? (from.schema ?? "") : bronzeOf(dataset.name),
    name: from ? from.table : (dataset.bronze ?? dataset.name),
    columns: listed ? [] : dataset.fields.map((f) => ({ name: f.column, data_type: "" })),
    datasets: [dataset.name],
    mark: listed ? "gone" : "read",
  };
}

/** What a dataset reads, said in a line: `source · schema.table`, or its bronze table. */
export function readsWhat(dataset: DatasetOut): string {
  const from = dataset.from;
  if (!from) return `${bronzeOf(dataset.name)}.${dataset.bronze ?? dataset.name}`;
  return `${from.source} · ${from.schema ? `${from.schema}.` : ""}${from.table}`;
}

/** The columns of a source's table some dataset's field reads, in lower case. */
export function readColumns(table: TableNode, datasets: DatasetOut[]): Set<string> {
  return new Set(
    datasets
      .filter((d) => table.datasets.includes(d.name))
      .flatMap((d) => d.fields.map((f) => f.column.toLowerCase())),
  );
}

/** Every rule of a dataset's files, by its file. */
function leaves(rules: RulesOut): (RuleLeaf & { field?: string | null; reads: string[] })[] {
  return rules.files.flatMap((f) =>
    f.rules.map((r) => ({ id: r.id, file: f.file, also: [], field: r.field, reads: r.reads })),
  );
}

/**
 * A table's columns as `dataset` reads them: each with its fields and, under
 * it, the rules that read them, a rule of several fields under each, marked
 * with the others; the columns its fields read that the table does not
 * have, gone; and the derived fields, each with its rules, apart.
 */
export function columnsOf(
  table: TableNode,
  dataset: DatasetOut,
  rules: RulesOut | undefined,
): { columns: ColumnNode[]; derived: DerivedNode[] } {
  const all = rules ? leaves(rules) : [];
  const mapped = new Set(dataset.fields.map((f) => f.field));
  const onColumn = all.filter((r) => !r.field || mapped.has(r.field));
  const leaf = (r: (typeof all)[number], field: string): RuleLeaf => ({
    id: r.id,
    file: r.file,
    also: r.reads.filter((f) => f !== field),
  });
  const node = (name: string, dataType: string | undefined): ColumnNode => {
    const fields = dataset.fields.filter((f) => same(f.column, name)).map((f) => f.field);
    const ruled = fields.flatMap((field) =>
      onColumn.filter((r) => r.reads.includes(field)).map((r) => leaf(r, field)),
    );
    const mark: Mark = !fields.length ? "unread" : dataType === undefined ? "gone" : "read";
    return { name, dataType, fields, rules: ruled, mark, gap: fields.length > 0 && !ruled.length };
  };
  const columns = table.columns.map((c) => node(c.name, c.data_type));
  for (const f of dataset.fields) {
    if (!columns.some((c) => same(c.name, f.column))) columns.push(node(f.column, undefined));
  }
  const derived = new Map<string, DerivedNode>();
  for (const r of all) {
    if (!r.field || mapped.has(r.field)) continue;
    const d = derived.get(r.field) ?? { field: r.field, from: r.reads, rules: [] };
    d.rules.push({ id: r.id, file: r.file, also: r.reads.filter((f) => !d.from.includes(f)) });
    derived.set(r.field, d);
  }
  return { columns, derived: [...derived.values()] };
}

/** Whether `name` holds every word of `query`, any case; an empty query, everything. */
export function matches(query: string, ...names: string[]): boolean {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  const text = names.join(" ").toLowerCase();
  return words.every((w) => text.includes(w));
}

/**
 * The schemas whose tables match `query` by schema, table or a column's
 * name, each with only those tables; all of them for an empty query.
 */
export function filterSchemas(schemas: SchemaNode[], query: string): SchemaNode[] {
  if (!query.trim()) return schemas;
  return schemas
    .map((s) => ({
      ...s,
      tables: s.tables.filter(
        (t) => matches(query, s.name, t.name) || t.columns.some((c) => matches(query, c.name)),
      ),
    }))
    .filter((s) => s.tables.length > 0);
}

/** Where a table's properties open, and a column's among them; a table of the lake has no source. */
export function propertiesPath(at: {
  source?: string;
  schema: string;
  table: string;
  column?: string;
}): string {
  const q = new URLSearchParams();
  if (at.source) q.set("source", at.source);
  q.set("schema", at.schema);
  q.set("table", at.table);
  if (at.column) q.set("column", at.column);
  return `/quality/table?${q.toString()}`;
}

/** The table `schema.table` among `schemas`, in any case. */
export function findTable(
  schemas: SchemaNode[],
  schema: string,
  table: string,
): TableNode | undefined {
  return schemas.find((s) => same(s.name, schema))?.tables.find((t) => same(t.name, table));
}
