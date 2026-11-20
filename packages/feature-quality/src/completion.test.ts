import { describe, expect, it } from "vitest";
import schema from "../../../crates/quality-rules-model/rules.schema.json";
import { context, ITEM, suggest } from "./completion";

/** `text` with `|` where the cursor is. */
function at(text: string) {
  const pos = text.indexOf("|");
  const ctx = context(text.replace("|", ""), pos);
  if (!ctx) throw new Error("no context");
  return ctx;
}

const labels = (text: string, fields: string[] = []) =>
  suggest(schema, at(text), fields).map((s) => s.label);

const RULE = "dataset: d\nrules:\n  - id: R-01\n";

describe("context", () => {
  it("finds a rule's key under its list item", () => {
    expect(at(`${RULE}    fie|`)).toMatchObject({
      path: ["rules", ITEM],
      at: "key",
      word: "fie",
      siblings: ["id"],
    });
  });

  it("finds a value, also on the item's first line", () => {
    expect(at(`${RULE}    check: re|`)).toMatchObject({ at: "value", key: "check", word: "re" });
    expect(at("rules:\n  - check: |")).toMatchObject({ path: ["rules", ITEM], key: "check" });
  });

  it("walks past sibling rules and nested keys", () => {
    const text = `${RULE}    examples:\n      fails:\n        - Id: x\n  - id: R-02\n    sev|`;
    expect(at(text)).toMatchObject({ path: ["rules", ITEM], siblings: ["id"] });
    expect(at(`${RULE}    examples:\n      |`).path).toEqual(["rules", ITEM, "examples"]);
  });

  it("completes nothing inside a block value", () => {
    expect(context(`${RULE}    fails: >-\n      Kin`, RULE.length + 22)).toBeUndefined();
    expect(at(`${RULE}    fails: >-\n      Kind = 1\n    sev|`).at).toBe("key");
  });

  it("completes nothing inside a value being written freely", () => {
    expect(context("rules:\n  - fails: a > |", 20)).toBeUndefined();
  });
});

describe("suggest", () => {
  it("offers a rule's keys not yet written", () => {
    const keys = labels(`${RULE}    |`);
    expect(keys).toContain("check");
    expect(keys).toContain("severity");
    expect(keys).not.toContain("id");
    expect(labels(`${RULE}    sev|`)).toEqual(["severity"]);
  });

  it("offers the top level's keys", () => {
    expect(labels("dataset: d\n|")).toContain("rules");
    expect(labels("dataset: d\n|")).not.toContain("dataset");
  });

  it("offers a key's allowed values", () => {
    expect(labels(`${RULE}    check: |`)).toContain("required");
    expect(labels(`${RULE}    severity: m|`)).toEqual(["major", "minor"]);
    expect(labels(`${RULE}    normalize: |`)).toEqual(["true", "false"]);
  });

  it("offers the mapping's fields for a rule's field", () => {
    expect(labels(`${RULE}    field: N|`, ["Id", "Name", "Nit"])).toEqual(["Name", "Nit"]);
  });

  it("offers an example's keys", () => {
    expect(labels(`${RULE}    examples:\n      |`)).toEqual(["fails", "passes", "skips"]);
  });
});
