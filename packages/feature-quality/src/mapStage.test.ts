import { describe, expect, it } from "vitest";
import type { MapProgress } from "./api";
import { stage } from "./mapStage";

const cut = (state: string, extra: object = {}) =>
  ({ cut: "2026-11-04", state, records: null, failed_at: null, error: null, ...extra }) as never;

const at = (p: Partial<MapProgress>) =>
  stage({ dataset: "loans", state: "done", error: null, cut: null, ...p });

describe("a map's stage", () => {
  it("is mapping until its job is done and its cut is there", () => {
    expect(at({ state: "queued" })).toEqual({ at: "queued" });
    expect(at({ state: "running" })).toEqual({ at: "mapping" });
    expect(at({ cut: null })).toEqual({ at: "mapping" });
  });

  it("collects while its cut extracts, then evaluates", () => {
    expect(at({ cut: cut("extracting") })).toEqual({ at: "extracting" });
    expect(at({ cut: cut("evaluating") })).toEqual({ at: "evaluating" });
  });

  it("is ready with its records once evaluated", () => {
    expect(at({ cut: cut("done", { records: 1200 }) })).toEqual({ at: "ready", records: 1200 });
  });

  it("says which step failed and why", () => {
    expect(at({ state: "failed", error: "no table" })).toEqual({
      at: "failed",
      step: "map",
      error: "no table",
    });
    expect(at({ cut: cut("failed", { failed_at: "extract", error: "timeout" }) })).toEqual({
      at: "failed",
      step: "extract",
      error: "timeout",
    });
  });
});
