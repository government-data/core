import { createRoot } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";
import { QualityError, rulesApi, type TestOut } from "./api";
import { forgetAll, keep, scopes } from "./reveal";
import { revealApi } from "./revealApi";
import {
  columns,
  conformity,
  createRuleTest,
  hasValues,
  lastTested,
  page,
  PAGE,
  ruleAt,
} from "./ruleTest";

const TEXT = [
  "dataset: loans", // 1
  "rules:", // 2
  "  - id: L-01", // 3
  "    field: Id", // 4
  "    examples:", // 5
  "      - id: not-a-rule", // 6
  "", // 7
  "  - field: Amount", // 8
  "    id: L-02", // 9
  "    check: required", // 10
  "lists:", // 11
  "  kinds: [a, b]", // 12
].join("\n");

describe("ruleAt", () => {
  it("finds the rule whose item holds the line, its id first or not", () => {
    expect(ruleAt(TEXT, 3)).toBe("L-01");
    expect(ruleAt(TEXT, 6)).toBe("L-01");
    expect(ruleAt(TEXT, 7)).toBe("L-01");
    expect(ruleAt(TEXT, 8)).toBe("L-02");
    expect(ruleAt(TEXT, 10)).toBe("L-02");
  });

  it("finds none outside the rules", () => {
    expect(ruleAt(TEXT, 1)).toBeUndefined();
    expect(ruleAt(TEXT, 2)).toBeUndefined();
    expect(ruleAt(TEXT, 12)).toBeUndefined();
    expect(ruleAt("dataset: d\n", 1)).toBeUndefined();
  });
});

describe("a read of records", () => {
  it("gives conformity over the records that apply, none when none does", () => {
    expect(conformity({ applies: 3, passes: 2 })).toBe(66.7);
    expect(conformity({ applies: 0, passes: 0 })).toBeUndefined();
  });

  it("lists the fields in the order they come, the key's first", () => {
    const value = (field: string) => ({ field, value: "x" });
    const records = [
      { outcome: "fails", values: [value("Id"), value("Amount")] },
      { outcome: "skips", values: [value("Id"), value("Kind")] },
    ];
    expect(columns(records)).toEqual(["Id", "Amount", "Kind"]);
  });

  it("knows whether a report lists any record, under a rule or a group", () => {
    const rule = { rule: "R-01", applies: 1, passes: 0, fails: 1, skips: 0, records: [] };
    const record = { outcome: "fails", values: [{ field: "Id", value: "x" }] };
    expect(hasValues({ rules: [] })).toBe(false);
    expect(hasValues({ rules: [rule] })).toBe(false);
    expect(hasValues({ rules: [{ ...rule, records: [record] }] })).toBe(true);
    const group = { shape: "s", count: 1, records: [record] };
    expect(hasValues({ rules: [{ ...rule, groups: [group] }] })).toBe(true);
  });

  it("shows a page of ten at a time", () => {
    const all = Array.from({ length: 23 }, (_, i) => i);
    expect(page(all, 0)).toHaveLength(PAGE);
    expect(page(all, 2)).toEqual([20, 21, 22]);
  });
});

const done: TestOut = {
  state: "done",
  masked: true,
  revealable: false,
  report: { cut: "2026-10-31", records: 3, rules: [] },
};

// A test done, as the editor shows it: masked, as its read answered.
const finished = (job: string, rule?: string) => ({
  phase: "done",
  rule,
  job,
  masked: done,
  out: done,
});

describe("createRuleTest", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it("queues the text and asks after it until it ends", async () => {
    vi.useFakeTimers();
    const start = vi.spyOn(rulesApi, "test").mockResolvedValue({ job: "j1", cut: "2026-10-31" });
    vi.spyOn(rulesApi, "readTest")
      .mockResolvedValueOnce({ state: "running", masked: true, revealable: false })
      .mockResolvedValueOnce(done);
    const tester = createRoot(() =>
      createRuleTest(
        () => "loans",
        () => "loans.yaml",
      ),
    );
    await tester.run("yaml", "L-01");
    expect(start).toHaveBeenCalledWith("loans", "yaml", "L-01", []);
    expect(tester.test()).toMatchObject({
      phase: "waiting",
      rule: "L-01",
      cut: "2026-10-31",
      state: "running",
    });
    await vi.advanceTimersByTimeAsync(2000);
    expect(tester.test()).toEqual(finished("j1", "L-01"));
    expect(tester.running()).toBe(false);
  });

  it("keeps each rule's last counts, a later test of it replacing them", async () => {
    const tested = (fails: number): TestOut => ({
      state: "done",
      masked: true,
      revealable: false,
      report: {
        cut: "2026-10-31",
        records: 3,
        rules: [{ rule: "D-01", applies: 3, passes: 3 - fails, fails, skips: 0, records: [] }],
      },
    });
    vi.spyOn(rulesApi, "test").mockResolvedValue({ job: "j3", cut: "2026-10-31" });
    const read = vi.spyOn(rulesApi, "readTest").mockResolvedValue(tested(2));
    const tester = createRoot(() =>
      createRuleTest(
        () => "deposits",
        () => "deposits.yaml",
      ),
    );
    expect(lastTested("deposits", "D-01")).toBeUndefined();
    await tester.run("yaml", "D-01");
    expect(lastTested("deposits", "D-01")).toEqual({ applies: 3, fails: 2 });
    read.mockResolvedValue(tested(0));
    await tester.run("yaml");
    expect(lastTested("deposits", "D-01")).toEqual({ applies: 3, fails: 0 });
    expect(lastTested("loans", "D-01")).toBeUndefined();
  });

  it("keeps a file's last test for when it is opened again", async () => {
    vi.spyOn(rulesApi, "test").mockResolvedValue({ job: "j2", cut: "2026-10-31" });
    vi.spyOn(rulesApi, "readTest").mockResolvedValue(done);
    const first = createRoot(() =>
      createRuleTest(
        () => "cards",
        () => "cards.yaml",
      ),
    );
    await first.run("yaml");
    const again = createRoot(() =>
      createRuleTest(
        () => "cards",
        () => "cards.yaml",
      ),
    );
    expect(again.test()).toEqual(finished("j2"));
    const other = createRoot(() =>
      createRuleTest(
        () => "cards",
        () => "other.yaml",
      ),
    );
    expect(other.test().phase).toBe("idle");
  });

  it("names why a test was refused", async () => {
    vi.spyOn(rulesApi, "test").mockRejectedValue(new QualityError(409, { error: "no_cut" }));
    const tester = createRoot(() =>
      createRuleTest(
        () => "loans",
        () => "loans.yaml",
      ),
    );
    await tester.run("yaml");
    expect(tester.test()).toEqual({ phase: "refused", rule: undefined, reason: "no_cut" });
  });
});

describe("a test whose result is no longer kept", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
    vi.useRealTimers();
  });

  it("stops asking and forgets the saved test once its result is no longer kept", async () => {
    vi.useFakeTimers();
    const kept = new Map<string, string>();
    vi.stubGlobal("sessionStorage", {
      getItem: (k: string) => kept.get(k) ?? null,
      setItem: (k: string, v: string) => void kept.set(k, v),
      removeItem: (k: string) => void kept.delete(k),
    });
    vi.spyOn(rulesApi, "test").mockResolvedValue({ job: "j5", cut: "2026-10-31" });
    const read = vi
      .spyOn(rulesApi, "readTest")
      .mockResolvedValueOnce({ state: "running", masked: true, revealable: false })
      .mockResolvedValue({ state: "expired", masked: true, revealable: false });
    const tester = createRoot(() =>
      createRuleTest(
        () => "loans",
        () => "expired.yaml",
      ),
    );
    await tester.run("yaml", "L-02");
    expect(kept.has("qd.editor.test.loans/expired.yaml")).toBe(true);
    await vi.advanceTimersByTimeAsync(2000);
    expect(tester.test()).toEqual({ phase: "expired", rule: "L-02" });
    expect(kept.has("qd.editor.test.loans/expired.yaml")).toBe(false);
    await vi.advanceTimersByTimeAsync(10_000);
    expect(read).toHaveBeenCalledTimes(2);
    expect(tester.running()).toBe(false);
  });
});

describe("a test's values shown whole (decision 0069)", () => {
  afterEach(() => {
    forgetAll();
    vi.restoreAllMocks();
  });

  it("shows a test masked first, then whole once with the dataset's kept reason", async () => {
    const read: TestOut = { ...done, revealable: true };
    const whole: TestOut = { ...read, masked: false };
    vi.spyOn(rulesApi, "test").mockResolvedValue({ job: "j4", cut: "2026-10-31" });
    vi.spyOn(rulesApi, "readTest").mockResolvedValue(read);
    const post = vi.spyOn(revealApi, "test").mockResolvedValue(whole);
    const tester = createRoot(() =>
      createRuleTest(
        () => "savings",
        () => "savings.yaml",
      ),
    );
    await tester.run("yaml");
    expect(post).not.toHaveBeenCalled();
    expect(tester.test()).toMatchObject({ phase: "done", out: read });

    const reason = { purpose: "rule_writing" as const, reference: "T-48213" };
    tester.showWhole(reason);
    await vi.waitFor(() => expect(tester.test()).toMatchObject({ out: whole, masked: read }));
    expect(post).toHaveBeenCalledTimes(1);
    expect(post).toHaveBeenCalledWith("j4", reason);
    tester.mask();
    expect(tester.test()).toMatchObject({ out: read });

    // A next test of the dataset goes with the reason kept, once.
    keep(scopes.dataset("savings"), reason);
    await tester.run("yaml");
    expect(post).toHaveBeenCalledTimes(2);
    expect(tester.test()).toMatchObject({ out: whole });
  });
});
