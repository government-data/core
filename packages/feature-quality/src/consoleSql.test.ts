import { describe, expect, it } from "vitest";
import { formatted, schemaQuery, tableQuery } from "./consoleSql";

describe("the console's statements", () => {
  it("lays a statement out as DuckDB reads it", () => {
    expect(formatted("select a,b from lake.x.y where a=1 limit 5")).toBe(
      "SELECT\n  a,\n  b\nFROM\n  lake.x.y\nWHERE\n  a = 1\nLIMIT\n  5",
    );
  });

  it("proposes a lake table's records, its names quoted", () => {
    expect(tableQuery("loans_bronze", 'we"ird')).toBe(
      'SELECT\n  *\nFROM\n  lake."loans_bronze"."we""ird"\nLIMIT\n  100',
    );
  });

  it("proposes a lake namespace's tables, its name quoted", () => {
    expect(schemaQuery("o'brien_bronze")).toContain("schema_name = 'o''brien_bronze'");
  });
});
