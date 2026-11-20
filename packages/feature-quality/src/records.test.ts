import { describe, expect, it } from "vitest";
import { QualityError } from "./api";
import { askFor, ready, refusal } from "./records";

describe("askFor", () => {
  it("asks the first page, the last ones descending, or page n in an order", () => {
    expect(askFor("dbo.t", "first", "", 3)).toEqual({
      table: "dbo.t",
      order: null,
      descending: false,
      page: 0,
    });
    expect(askFor("dbo.t", "last", "id", 3)).toMatchObject({
      order: "id",
      descending: true,
      page: 0,
    });
    expect(askFor("dbo.t", "all", "id", 3)).toMatchObject({ descending: false, page: 3 });
  });

  it("needs a column for the last ones, and names a refusal", () => {
    expect(ready("last", "")).toBe(false);
    expect(ready("all", "")).toBe(true);
    expect(refusal(new QualityError(422, { error: "order" }))).toBe("order");
    expect(refusal(new QualityError(403, null))).toBe("forbidden");
    expect(refusal(new Error("x"))).toBe("other");
  });
});
