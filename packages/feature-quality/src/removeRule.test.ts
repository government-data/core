import { describe, expect, it } from "vitest";
import { withoutRule } from "./removeRule";

const FILE = `dataset: loans
rules:
  - id: id-01
    field: id
    check: required # kept

  - id: id-02
    field: id
    check: unique
`;

describe("a rules file without a rule", () => {
  it("takes out its item and keeps the rest as written", () => {
    expect(withoutRule(FILE, "id-01")).toBe(`dataset: loans
rules:
  - id: id-02
    field: id
    check: unique
`);
    expect(withoutRule(FILE, "id-02")).toBe(`dataset: loans
rules:
  - id: id-01
    field: id
    check: required # kept

`);
  });

  it("leaves an empty list when it was the last one", () => {
    const one = "dataset: loans\nrules:\n  - id: id-01\n    field: id\n    check: required\n";
    expect(withoutRule(one, "id-01")).toBe("dataset: loans\nrules: []\n");
  });

  it("is nothing for an id no item has", () => {
    expect(withoutRule(FILE, "R-01.id")).toBeUndefined();
  });
});
