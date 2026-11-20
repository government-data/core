import { describe, expect, it } from "vitest";
import type { DraftOut } from "./api";
import {
  approvable,
  approvalRefusal,
  authorName,
  columnOf,
  errorCount,
  parseBranch,
} from "./drafts";

const draft = (errors: string[][], reads = true): DraftOut => ({
  branch: "proposal/clientes/job-1",
  commit: "abc",
  mapping: "dataset: clientes\n",
  mapping_reads: reads,
  rules: errors.map((e, i) => ({ file: `group_${i}.yaml`, yaml: "rules: []\n", errors: e })),
});

describe("parseBranch", () => {
  it("reads a proposal's dataset and id", () => {
    expect(parseBranch("proposal/clientes/0193-ab")).toEqual({
      dataset: "clientes",
      id: "0193-ab",
    });
  });

  it("refuses anything else", () => {
    expect(parseBranch("main")).toBeUndefined();
    expect(parseBranch("proposal/clientes")).toBeUndefined();
    expect(parseBranch("draft/clientes/1")).toBeUndefined();
    expect(parseBranch("proposal/clientes/1/2")).toBeUndefined();
  });
});

describe("approvable", () => {
  it("needs a mapping that reads and no lint error", () => {
    expect(approvable(draft([[], []]))).toBe(true);
    expect(approvable(draft([[], ["unknown_field"]]))).toBe(false);
    expect(approvable(draft([[]], false))).toBe(false);
    expect(errorCount(draft([["a", "b"], ["c"]]))).toBe(3);
  });
});

describe("approvalRefusal", () => {
  it("names each refusal", () => {
    expect(approvalRefusal(409, { error: "stale" })).toBe("stale");
    expect(approvalRefusal(409, { error: "conflicts", tables: ["rules"] })).toBe("conflicts");
    expect(approvalRefusal(422, { error: "unknown_source" })).toBe("unknownSource");
    expect(approvalRefusal(422, draft([["x"]]))).toBe("lint");
    expect(approvalRefusal(403, { error: "forbidden" })).toBe("forbidden");
    expect(approvalRefusal(404, undefined)).toBe("notFound");
    expect(approvalRefusal(503, { error: "catalog_unavailable" })).toBe("other");
  });
});

describe("authorName", () => {
  it("shows the name, an account id shortened", () => {
    expect(authorName("Map job <map@example.org>")).toBe("Map job");
    expect(
      authorName(
        "4fc41d54-6b44-4904-aa5a-c4c49fde2929 <4fc41d54-6b44-4904-aa5a-c4c49fde2929@accounts.invalid>",
      ),
    ).toBe("4fc41d54…");
    expect(authorName("<only@example.org>")).toBe("<only@example.org>");
  });
});

describe("columnOf", () => {
  const change = (file: string) => ({ file, before: null, after: "x" });
  it("is the one rules file a draft changes, or none for a draft of the dataset", () => {
    expect(columnOf([change("amount.yaml")])).toBe("amount.yaml");
    expect(columnOf([change("mapping")])).toBeUndefined();
    expect(columnOf([change("amount.yaml"), change("id.yaml")])).toBeUndefined();
    expect(columnOf([])).toBeUndefined();
  });
});
