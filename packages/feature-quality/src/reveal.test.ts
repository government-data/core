import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { signedOut } from "@knime-orchestador/feature-kit";
import { QualityError } from "./apiCore";
import { t } from "./i18n";
import {
  barState,
  forget,
  forgetAll,
  keep,
  kept,
  masked,
  reasonOf,
  referenceProblem,
  reveal,
  revealRefusal,
  scopes,
  type Revealing,
} from "./reveal";
// The feature as the shell registers it: its forgetting at sign-out with it.
import "./index";

/** The tab's storage, as a browser keeps it. */
function fakeStorage() {
  const items = new Map<string, string>();
  return {
    get length() {
      return items.size;
    },
    key: (i: number) => [...items.keys()][i] ?? null,
    getItem: (k: string) => items.get(k) ?? null,
    setItem: (k: string, v: string) => void items.set(k, v),
    removeItem: (k: string) => void items.delete(k),
    items,
  };
}

let storage: ReturnType<typeof fakeStorage>;
beforeEach(() => {
  storage = fakeStorage();
  vi.stubGlobal("sessionStorage", storage);
});
afterEach(() => {
  forgetAll();
  vi.unstubAllGlobals();
});

describe("the reference", () => {
  it("accepts a ticket or case number", () => {
    for (const r of ["INC-2041", "CASO/2026/118", "T-48213", "a", "X".repeat(32)])
      expect(referenceProblem("quality_review", r)).toBeUndefined();
  });

  it("refuses a space, digits only, more than 32, a letter outside ASCII, a leading sign", () => {
    for (const r of ["INC 2041", "12345678", "X".repeat(33), "CASOÑ-1", "-INC1", "#A1"])
      expect(referenceProblem("audit", r)).toBe("reference");
  });

  it("is optional but with other, and an empty one is absent", () => {
    expect(referenceProblem("remediation", "  ")).toBeUndefined();
    expect(referenceProblem("other", "")).toBe("required");
    expect(referenceProblem("other", " INC-1 ")).toBeUndefined();
    expect(reasonOf("audit", "  ")).toEqual({ purpose: "audit", reference: null });
    expect(reasonOf("other", " INC-1 ")).toEqual({ purpose: "other", reference: "INC-1" });
  });
});

describe("a reason kept in the tab", () => {
  const review = { purpose: "quality_review" as const, reference: "INC-2041" };

  it("is kept per scope, in the tab's storage", () => {
    keep(scopes.source("core"), review);
    expect(kept("source/core")).toEqual(review);
    expect(kept(scopes.source("other"))).toBeUndefined();
    expect(kept(scopes.dataset("loans"))).toBeUndefined();
    expect(JSON.parse(storage.getItem("qd.reveal.source/core") ?? "null")).toEqual(review);
  });

  it("is forgotten by Mask again, for its scope only", () => {
    keep(scopes.lake, review);
    keep(scopes.dataset("loans"), review);
    forget(scopes.lake);
    expect(kept(scopes.lake)).toBeUndefined();
    expect(storage.getItem("qd.reveal.lake")).toBeNull();
    expect(kept("pipeline/loans")).toEqual(review);
  });

  it("is forgotten at sign-out, every scope, nothing else of the tab", () => {
    storage.setItem("qd.editor.open", "[]");
    keep(scopes.lake, review);
    keep(scopes.source("core"), review);
    signedOut();
    expect(kept(scopes.lake)).toBeUndefined();
    expect(kept(scopes.source("core"))).toBeUndefined();
    expect([...storage.items.keys()]).toEqual(["qd.editor.open"]);
  });

  it("survives a reload from the tab, and a stored one not valid is not used", () => {
    storage.setItem("qd.reveal.lake", JSON.stringify(review));
    expect(kept(scopes.lake)).toEqual(review);
    storage.setItem("qd.reveal.source/x", JSON.stringify({ purpose: "other", reference: null }));
    expect(kept("source/x")).toBeUndefined();
  });
});

describe("a refused reveal", () => {
  it("names each of the API's codes, with a text in both languages", () => {
    const cases: [number, string][] = [
      [403, "forbidden"],
      [409, "not_done"],
      [422, "purpose"],
      [422, "reference"],
      [503, "audit_unavailable"],
      [503, "unavailable"],
    ];
    for (const [status, error] of cases)
      expect(revealRefusal(new QualityError(status, { error }))).toBe(error);
    expect(revealRefusal(new QualityError(404, { error: "not_found" }))).toBe("not_found");
    expect(revealRefusal(new QualityError(500, null))).toBe("other");
    expect(revealRefusal(new Error("x"))).toBe("other");
    expect(t("quality:reveal.refused.audit_unavailable")).toContain("auditoría");
    for (const code of [...cases.map((c) => c[1]), "not_found", "other"])
      expect(t(`quality:reveal.refused.${code}`)).not.toContain("reveal.refused");
  });
});

describe("revealing a view", () => {
  type Out = { revealable: boolean; masked: boolean; rows: string[] };
  const read: Out = { revealable: true, masked: true, rows: ["••••1234"] };
  const whole: Out = { revealable: true, masked: false, rows: ["5551234"] };
  const view = (job: string, out = read): Revealing<Out> => ({ job, masked: out, out });
  const reason = { purpose: "rule_writing" as const, reference: null };

  it("posts nothing without a kept reason, or when the read may not be revealed", async () => {
    const post = vi.fn(async () => whole);
    const set = vi.fn();
    await reveal("source/core", view("j1"), post, set);
    keep("source/core", reason);
    await reveal("source/core", view("j2", { ...read, revealable: false }), post, set);
    expect(post).not.toHaveBeenCalled();
    expect(set).not.toHaveBeenCalled();
  });

  it("uses the kept reason for each next page, one post per page", async () => {
    keep("source/core", reason);
    const post = vi.fn(async () => whole);
    const shown: Revealing<Out>[] = [];
    await reveal("source/core", view("p0"), post, (v) => shown.push(v));
    await reveal("source/core", view("p1"), post, (v) => shown.push(v));
    expect(post.mock.calls).toEqual([[reason], [reason]]);
    expect(shown.filter((v) => !v.busy).map((v) => [v.job, v.out])).toEqual([
      ["p0", whole],
      ["p1", whole],
    ]);
    expect(masked(shown[1])).toMatchObject({ out: read, refused: undefined });
  });

  it("says why it was refused, forgetting a reason the API does not accept", async () => {
    keep(scopes.lake, { purpose: "audit", reference: "INC-1" });
    const shown: Revealing<Out>[] = [];
    const refuse = (error: string) => async () => {
      throw new QualityError(error === "reference" ? 422 : 503, { error });
    };
    await reveal(scopes.lake, view("c1"), refuse("audit_unavailable"), (v) => shown.push(v));
    expect(shown.at(-1)).toMatchObject({ out: read, refused: "audit_unavailable" });
    expect(kept(scopes.lake)).toBeDefined();
    await reveal(scopes.lake, view("c1"), refuse("reference"), (v) => shown.push(v));
    expect(shown.at(-1)?.refused).toBe("reference");
    expect(kept(scopes.lake)).toBeUndefined();
  });
});

describe("the bar above values", () => {
  it("says values are shown only while they are whole", () => {
    expect(barState(true, true, false)).toBe("showing");
    expect(barState(false, true, false)).toBe("none");
    expect(barState(false, true, true)).toBe("ask");
    expect(barState(false, false, true)).toBe("none");
  });

  it("after a refused reveal, with a reason kept, shows them masked with Change", () => {
    expect(barState(true, true, true)).toBe("retry");
    expect(barState(true, false, true)).toBe("none");
    expect(t("quality:reveal.forget")).not.toContain("reveal.forget");
  });
});
