import { describe, expect, it } from "vitest";
import { paint, type Kind } from "./paint";

/** The words of `text` painted `kind`. */
function words(text: string, kind: Kind, fields = ["Id", "Kind", "Name"]) {
  return paint(text, fields)
    .filter((p) => p.kind === kind)
    .map((p) => text.slice(p.from, p.to));
}

const RULE = `dataset: d
rules:
  - id: R-01
    field: Name
    check: "expression"
    fails: "Kind IN ('A', 'B') OR coalesce(Name, '') LIKE 'X%' AND Since > :cut"
    severity: major
`;

describe("paint", () => {
  it("paints a rule's fields and allowed values", () => {
    expect(words(RULE, "field")).toEqual(["Name", "Kind", "Name"]);
    expect(words(RULE, "value")).toEqual(["expression", "major"]);
  });

  it("paints an expression's SQL", () => {
    expect(words(RULE, "keyword")).toEqual(["IN", "OR", "LIKE", "AND"]);
    expect(words(RULE, "function")).toEqual(["coalesce"]);
    expect(words(RULE, "text")).toEqual(["'A'", "'B'", "''", "'X%'"]);
    expect(words(RULE, "param")).toEqual([":cut"]);
    expect(words(RULE, "expr")).toHaveLength(1);
  });

  it("paints an expression written without quotes, commas and all", () => {
    const plain = RULE.replace(/fails: "(.*)"/, "fails: $1  # why");
    expect(words(plain, "keyword")).toEqual(["IN", "OR", "LIKE", "AND"]);
    expect(words(plain, "text")).toEqual(["'A'", "'B'", "''", "'X%'"]);
    expect(words(plain, "param")).toEqual([":cut"]);
    expect(words(plain, "field")).toEqual(["Name", "Kind", "Name"]);
  });

  it("paints an expression folded over several lines", () => {
    const folded = `derived:
  Total: >-
    CASE WHEN Kind = 'A'
    THEN Id + 1 END
rules:
  - id: R
    field: Total
    check: required
`;
    expect(words(folded, "keyword")).toEqual(["CASE", "WHEN", "THEN", "END"]);
    expect(words(folded, "field")).toEqual(["Total", "Kind", "Id", "Total"]);
    expect(words(folded, "value")).toEqual(["required"]);
  });

  it("paints a rule's when as an expression", () => {
    const text = RULE.replace(
      "    severity: major\n",
      "    when: Kind = 'A' AND coalesce(Name, '') <> 'B'\n    severity: major\n",
    );
    expect(words(text, "keyword")).toEqual(["IN", "OR", "LIKE", "AND", "AND"]);
    expect(words(text, "function")).toEqual(["coalesce", "coalesce"]);
  });

  it("leaves a name that is not a field unpainted", () => {
    expect(words(RULE, "field")).not.toContain("Since");
  });

  it("knows the derived fields and paints their expressions", () => {
    const text = 'derived:\n  Total: "Id + 1"\nrules:\n  - id: R\n    field: Total\n';
    expect(words(text, "field")).toEqual(["Total", "Id", "Total"]);
    expect(words(text, "number")).toEqual(["1"]);
  });

  it("paints an example's fields and an expression inside a flow map", () => {
    const text = `references:
  other: { dataset: o, key: Id, where: "Kind = 'A'" }
rules:
  - id: R
    examples:
      fails:
        - Name: x
    cause: capture
`;
    expect(words(text, "field")).toEqual(["Kind", "Name"]);
    expect(words(text, "text")).toEqual(["'A'"]);
    expect(words(text, "value")).toEqual(["capture"]);
  });
});
