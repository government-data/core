import { describe, expect, it } from "vitest";
import type { DatasetOut, ListingView, RulesOut, RuleView } from "./api";
import {
  columnsOf,
  databaseOf,
  filterSchemas,
  findTable,
  datasetTable,
  lakeSchemas,
  lakeTables,
  readsWhat,
  newDataset,
  propertiesPath,
  sourceSchemas,
  type TableNode,
} from "./explorer";

const col = (name: string, data_type = "VARCHAR") => ({ name, data_type });

const listing = (tables: ListingView["tables"], state: ListingView["state"] = "listed") =>
  ({ state, tables, truncated: false }) as ListingView;

const loans: DatasetOut = {
  name: "loans",
  from: { source: "core", schema: "public", table: "loans" },
  bronze: "loans",
  fields: [
    { field: "id", column: "ID" },
    { field: "amount", column: "amount" },
    { field: "branch", column: "branch_code" },
  ],
};

const rule = (id: string, field: string | null, reads: string[]) =>
  ({ id, field, reads }) as RuleView;

const rules = (...r: RuleView[]): RulesOut =>
  ({
    commit: "c",
    mapping_reads: true,
    files: [{ file: "loans.yaml", yaml: "", rules: r, findings: [] }],
  }) as RulesOut;

describe("sourceSchemas", () => {
  it("groups by schema and marks each table read or not, in any case", () => {
    const schemas = sourceSchemas(
      "core",
      listing([
        { schema: "public", name: "LOANS", columns: [] },
        { schema: "public", name: "audit", columns: [] },
        { schema: "acct", name: "ledger", columns: [] },
      ]),
      [loans],
    );
    expect(schemas.map((s) => [s.name, s.tables.map((t) => [t.name, t.mark, t.datasets])])).toEqual(
      [
        ["acct", [["ledger", "unread", []]]],
        [
          "public",
          [
            ["audit", "unread", []],
            ["LOANS", "read", ["loans"]],
          ],
        ],
      ],
    );
  });

  it("keeps a table a dataset reads and the listing lost, gone; nothing gone before a listing", () => {
    const gone = sourceSchemas("core", listing([{ schema: "public", name: "x", columns: [] }]), [
      loans,
    ]);
    expect(gone[0]!.tables.find((t) => t.name === "loans")).toMatchObject({
      mark: "gone",
      datasets: ["loans"],
    });
    expect(sourceSchemas("core", listing([], "none"), [loans])).toEqual([]);
    expect(sourceSchemas("other", listing([]), [loans])).toEqual([]);
  });
});

describe("lakeSchemas", () => {
  const byHand: DatasetOut = { name: "cards", from: null, bronze: "cards", fields: [] };
  const lake = listing([
    { schema: "loans_bronze", name: "loans", columns: [] },
    { schema: "cards_bronze", name: "cards", columns: [] },
    { schema: "misc", name: "raw", columns: [] },
  ]);

  it("lists only what no dataset reads: a dataset's bronze table is its node (0049)", () => {
    const schemas = lakeSchemas(lake, [loans, byHand]);
    expect(schemas.map((s) => [s.name, s.tables.map((t) => t.name)])).toEqual([["misc", ["raw"]]]);
  });

  it("still says, for a table's properties, which dataset each bronze table is", () => {
    const schemas = lakeTables(lake, [loans, byHand]);
    expect(schemas.map((s) => [s.name, s.tables.map((t) => t.datasets)])).toEqual([
      ["cards_bronze", [["cards"]]],
      ["loans_bronze", [["loans"]]],
      ["misc", [[]]],
    ]);
  });
});

describe("datasetTable", () => {
  it("is the source's table as listed, or the fields' columns when it is not listed", () => {
    const listed = listing([
      { schema: "public", name: "loans", columns: [col("loan_id", "INTEGER")] },
    ]);
    expect(datasetTable(loans, listed)).toMatchObject({ name: "loans", mark: "read" });
    const unlisted = datasetTable(loans, undefined);
    expect(unlisted.columns.map((c) => c.name)).toEqual(loans.fields.map((f) => f.column));
    expect(datasetTable(loans, listing([]))).toMatchObject({ mark: "gone", columns: [] });
    expect(readsWhat(loans)).toContain("core · ");
  });
});

describe("columnsOf", () => {
  const table: TableNode = {
    schema: "public",
    name: "loans",
    columns: [col("id", "INTEGER"), col("amount", "DECIMAL(14,2)"), col("notes")],
    datasets: ["loans"],
    mark: "read",
  };

  it("puts each rule under the columns it reads, marked with the other fields", () => {
    const { columns } = columnsOf(
      table,
      loans,
      rules(rule("id.required", "id", ["id"]), rule("amount_by_id", null, ["amount", "id"])),
    );
    const at = (name: string) => columns.find((c) => c.name === name)!;
    expect(at("id").rules).toEqual([
      { id: "id.required", file: "loans.yaml", also: [] },
      { id: "amount_by_id", file: "loans.yaml", also: ["amount"] },
    ]);
    expect(at("amount").rules).toEqual([{ id: "amount_by_id", file: "loans.yaml", also: ["id"] }]);
    expect(at("notes")).toMatchObject({ fields: [], mark: "unread", gap: false });
  });

  it("marks a field no rule checks a gap, and a column its field reads that is not there gone", () => {
    const { columns } = columnsOf(table, loans, rules());
    expect(columns.find((c) => c.name === "amount")).toMatchObject({ gap: true, mark: "read" });
    expect(columns.find((c) => c.name === "branch_code")).toMatchObject({
      fields: ["branch"],
      mark: "gone",
      dataType: undefined,
    });
  });

  it("keeps a derived field's rules apart, with the fields it comes from", () => {
    const { columns, derived } = columnsOf(
      table,
      loans,
      rules(rule("age.range", "age", ["id", "amount"])),
    );
    expect(derived).toEqual([
      {
        field: "age",
        from: ["id", "amount"],
        rules: [{ id: "age.range", file: "loans.yaml", also: [] }],
      },
    ]);
    expect(columns.flatMap((c) => c.rules)).toEqual([]);
  });
});

describe("filterSchemas", () => {
  const schemas = sourceSchemas(
    "core",
    listing([
      { schema: "public", name: "loans", columns: [col("amount")] },
      { schema: "acct", name: "ledger", columns: [col("entry")] },
    ]),
    [],
  );

  it("keeps the tables whose schema, name or a column holds every word", () => {
    expect(filterSchemas(schemas, "ACCT").map((s) => s.name)).toEqual(["acct"]);
    expect(filterSchemas(schemas, "public loans")[0]!.tables.map((t) => t.name)).toEqual(["loans"]);
    expect(filterSchemas(schemas, "amount")[0]!.tables.map((t) => t.name)).toEqual(["loans"]);
    expect(filterSchemas(schemas, "nothing")).toEqual([]);
    expect(filterSchemas(schemas, " ")).toBe(schemas);
  });
});

describe("a table's properties", () => {
  it("open at a path saying the source, or none for the lake, and the column", () => {
    expect(propertiesPath({ source: "core", schema: "public", table: "loans" })).toBe(
      "/quality/table?source=core&schema=public&table=loans",
    );
    expect(propertiesPath({ schema: "x_bronze", table: "x", column: "a b" })).toBe(
      "/quality/table?schema=x_bronze&table=x&column=a+b",
    );
  });

  it("find their table in any case", () => {
    const schemas = sourceSchemas(
      "core",
      listing([{ schema: "PUBLIC", name: "Loans", columns: [] }]),
      [],
    );
    expect(findTable(schemas, "public", "loans")?.name).toBe("Loans");
    expect(findTable(schemas, "public", "cards")).toBeUndefined();
  });
});

describe("databaseOf", () => {
  it("is a server's database, or Oracle's service", () => {
    expect(databaseOf({ host: "db.example", database: "core" })).toBe("core");
    expect(databaseOf({ host: "db.example", service: "orcl" })).toBe("orcl");
    expect(databaseOf({ host: "db.example", database: "" })).toBeUndefined();
  });
});

describe("newDataset", () => {
  const table = (schema: string, name: string): TableNode => ({
    schema,
    name,
    columns: [],
    datasets: [],
    mark: "unread",
  });
  it("is the lake namespace's dataset, or one named as a source's table", () => {
    expect(newDataset(undefined, table("advisors_bronze", "advisors"))).toEqual({
      dataset: "advisors",
      table: { table: "advisors" },
    });
    expect(newDataset("core", table("dbo", "Loan Officers"))).toEqual({
      dataset: "loan_officers",
      table: { source: "core", table: "dbo.Loan Officers" },
    });
  });
  it("is none when no name would do", () => {
    expect(newDataset(undefined, table("raw", "advisors"))).toBeUndefined();
    expect(newDataset("core", table("dbo", "2024"))).toBeUndefined();
  });
});
