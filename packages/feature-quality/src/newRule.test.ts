import { describe, expect, it } from "vitest";
import { emptyFile, fieldFile, freeId, GLOBAL_FILE, withNewRule } from "./newRule";

describe("withNewRule", () => {
  it("adds the rule at the end of the rules list, before what follows it", () => {
    const yaml =
      "dataset: d\nrules:\n  - id: A-01\n    field: Amount\n    check: required\n\npopulations: {}\n";
    const out = withNewRule(yaml, "Email");
    expect(out.yaml).toBe(
      "dataset: d\nrules:\n  - id: A-01\n    field: Amount\n    check: required\n" +
        "  - id: Email-01\n    field: Email\n    check: required\n\npopulations: {}\n",
    );
    expect(out.line).toBe(6);
    expect(out.yaml.split("\n")[out.line - 1]).toBe("  - id: Email-01");
  });

  it("keeps the list's indentation", () => {
    const out = withNewRule("rules:\n- id: A-01\n  field: Amount\n  check: required\n", "Email");
    expect(out.yaml).toBe(
      "rules:\n- id: A-01\n  field: Amount\n  check: required\n- id: Email-01\n  field: Email\n  check: required\n",
    );
  });

  it("starts the list when the file has none, or an empty one", () => {
    expect(withNewRule("dataset: d\n", "Email").yaml).toBe(
      "dataset: d\nrules:\n  - id: Email-01\n    field: Email\n    check: required\n",
    );
    expect(withNewRule("dataset: d\nrules: []\n", "Email").line).toBe(3);
  });
});

describe("freeId", () => {
  it("gives the field's next id no rule has", () => {
    expect(freeId('rules:\n  - id: "Email-01"\n  - id: Email-02 # x\n', "Email")).toBe("Email-03");
  });
});

describe("fieldFile", () => {
  it("is the field's name as a file, if it can be one", () => {
    expect(fieldFile("IDClientes_2")).toBe("IDClientes_2.yaml");
    expect(fieldFile("Código")).toBeUndefined();
    expect(fieldFile("a/b")).toBeUndefined();
  });

  it("starts empty, and a rule goes in its list", () => {
    expect(withNewRule(emptyFile("loans"), "Id").yaml).toBe(
      "dataset: loans\nrules:\n  - id: Id-01\n    field: Id\n    check: required\n",
    );
  });
});

describe("emptyFile", () => {
  it("starts the table's global rules with no rules list", () => {
    expect(emptyFile("loans", GLOBAL_FILE)).toBe("dataset: loans\n");
    expect(emptyFile("loans", "Id.yaml")).toBe("dataset: loans\nrules: []\n");
  });
});
