import { afterEach, describe, expect, it, vi } from "vitest";
import { consoleApi, type ConsoleOut } from "./api";
import { current, maskAgain, runStatement, showWhole } from "./consoleStore";
import { forgetAll, keep, scopes } from "./reveal";
import { revealApi } from "./revealApi";

afterEach(() => {
  forgetAll();
  vi.restoreAllMocks();
  vi.useRealTimers();
});

// Synthetic rows: no real value.
const rows = (value: string) => ({ columns: ["code"], rows: [[value]], more: false });
const read: ConsoleOut = {
  state: "done",
  masked: true,
  revealable: true,
  result: rows("••••0001"),
};
const whole: ConsoleOut = { ...read, masked: false, result: rows("A-0001") };

describe("a console result", () => {
  it("is masked first, polling posts no reveal, and _Show values_ posts one", async () => {
    vi.useFakeTimers();
    vi.spyOn(consoleApi, "run").mockResolvedValue({ job: "c1" });
    vi.spyOn(consoleApi, "read")
      .mockResolvedValueOnce({ state: "running", masked: true, revealable: false })
      .mockResolvedValue(read);
    const post = vi.spyOn(revealApi, "console").mockResolvedValue(whole);
    await runStatement("SELECT 1");
    await vi.advanceTimersByTimeAsync(3000);
    expect(current()?.ran).toMatchObject({ phase: "done", out: read });
    expect(post).not.toHaveBeenCalled();

    const reason = { purpose: "audit" as const, reference: null };
    await showWhole(current()?.id ?? 0, reason);
    expect(post).toHaveBeenCalledExactlyOnceWith("c1", reason);
    expect(current()?.ran).toMatchObject({ out: whole });
    maskAgain(current()?.id ?? 0);
    expect(current()?.ran).toMatchObject({ out: read });
  });

  it("goes whole with the lake's kept reason, once per statement", async () => {
    vi.useFakeTimers();
    keep(scopes.lake, { purpose: "quality_review", reference: "INC-2041" });
    vi.spyOn(consoleApi, "run").mockResolvedValue({ job: "c2" });
    vi.spyOn(consoleApi, "read").mockResolvedValue(read);
    const post = vi.spyOn(revealApi, "console").mockResolvedValue(whole);
    await runStatement("SELECT 2");
    await vi.advanceTimersByTimeAsync(3000);
    expect(post).toHaveBeenCalledExactlyOnceWith("c2", {
      purpose: "quality_review",
      reference: "INC-2041",
    });
    expect(current()?.ran).toMatchObject({ out: whole });
  });
});
