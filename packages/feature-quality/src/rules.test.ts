import { describe, expect, it } from "vitest";
import type { RulesOut, RuleView } from "./api";
import { broken, keep, NO_FILTER, options, rows } from "./rules";
import { BY_ID, sorted } from "./rules";

const rule = (id: string, field: string, pattern: string, dimension: string): RuleView => ({
  id,
  field,
  reads: [field],
  pattern,
  kind: "de campo",
  dimension,
  severity: "mayor",
  applies: "todos los registros",
  fails: `${field} está vacío`,
  skips: null,
  duckdb: "SELECT 1",
  trino: "SELECT 1",
});

const out: RulesOut = {
  commit: "c1",
  mapping_reads: true,
  files: [
    {
      file: "basic.yaml",
      yaml: "",
      rules: [
        rule("L-01", "Id", "required", "completitud"),
        rule("L-02", "Fecha", "fresh", "oportunidad"),
      ],
      findings: [],
    },
    {
      file: "codes.yaml",
      yaml: "",
      rules: [rule("L-03", "Codigo", "format", "validez")],
      findings: [
        { code: "QR003", severity: "error", rule: "L-09", line: 4, column: 12, message: "x" },
        { code: "QR100", severity: "warning", rule: null, line: null, column: null, message: "y" },
      ],
    },
  ],
};

describe("rules", () => {
  it("lists every rule with its file", () => {
    expect(rows(out).map((r) => `${r.file}:${r.id}`)).toEqual([
      "basic.yaml:L-01",
      "basic.yaml:L-02",
      "codes.yaml:L-03",
    ]);
  });

  it("names the files with errors, warnings aside", () => {
    expect(broken(out)).toEqual([{ file: "codes.yaml", errors: [out.files[1].findings[0]] }]);
  });

  it("offers each value once, sorted", () => {
    expect(options(rows(out), "pattern")).toEqual(["format", "fresh", "required"]);
  });

  it("keeps the rules a filter asks for", () => {
    const all = rows(out);
    expect(keep(all, NO_FILTER)).toHaveLength(3);
    expect(keep(all, { ...NO_FILTER, pattern: "fresh" }).map((r) => r.id)).toEqual(["L-02"]);
    expect(keep(all, { ...NO_FILTER, text: "codigo" }).map((r) => r.id)).toEqual(["L-03"]);
    expect(keep(all, { ...NO_FILTER, text: "l-01" }).map((r) => r.id)).toEqual(["L-01"]);
    expect(keep(all, { ...NO_FILTER, dimension: "validez", pattern: "required" })).toEqual([]);
  });
});

describe("sorted", () => {
  const row = (id: string, field: string | null) => ({ id, field, file: "f.yaml" }) as never;
  const all = [row("V-PN-10", "B"), row("P-20", null), row("V-PN-2", "A"), row("V-PN-01", "B")];
  it("orders by id as a person counts, or by a column with its ties by id", () => {
    expect(sorted(all, BY_ID).map((r) => r.id)).toEqual(["P-20", "V-PN-01", "V-PN-2", "V-PN-10"]);
    expect(sorted(all, { key: "field", descending: false }).map((r) => r.id)).toEqual([
      "V-PN-2",
      "V-PN-01",
      "V-PN-10",
      "P-20",
    ]);
    expect(sorted(all, { key: "id", descending: true })[0]?.id).toBe("V-PN-10");
  });
});
