import { describe, expect, it } from "vitest";
import type { UpdatesView } from "./api";
import { elapsed, updateProgress } from "./updateProgress";

const now = new Date("2026-10-18T10:10:00Z");

function view(report: Partial<UpdatesView["report"]>, extra: Partial<UpdatesView> = {}) {
  return {
    daemon: true,
    reported_at: null,
    stale: false,
    report: {
      phase: "idle",
      installed: "1.0.4",
      available: null,
      target: null,
      failed: [],
      last: null,
      handled_request: null,
      warning: null,
      ...report,
    },
    pending_request: null,
    ...extra,
  } as unknown as UpdatesView;
}

describe("updateProgress", () => {
  it("is nothing when idle", () => {
    expect(updateProgress(view({}), now)).toBeNull();
  });

  it("starts with the request", () => {
    const p = updateProgress(
      view({}, { pending_request: { id: "1", at: "", by: null } } as never),
      now,
    );
    expect(p?.label).toBe("Pedido enviado");
    expect(p?.percent).toBeLessThan(10);
  });

  it("advances by phase and tells what the daemon reports", () => {
    const drain = updateProgress(
      view({
        phase: "draining",
        progress: "2 flujos corriendo",
        phase_since: "2026-10-18T10:07:00Z",
      }),
      now,
    )!;
    expect(drain.detail).toBe("2 flujos corriendo · 3 min");
    const apply = updateProgress(view({ phase: "applying" }), now)!;
    const verify = updateProgress(view({ phase: "verifying" }), now)!;
    expect(drain.percent).toBeLessThan(apply.percent);
    expect(apply.percent).toBeLessThan(verify.percent);
    expect(apply.tone).toBe("run");
  });

  it("warns while going back, and a restarting API is expected", () => {
    expect(updateProgress(view({ phase: "rolling_back" }), now)?.tone).toBe("warn");
    expect(updateProgress(view({ phase: "applying" }), now, true)?.label).toContain("reiniciando");
  });
});

describe("elapsed", () => {
  it("reads like a person would say it", () => {
    expect(elapsed("2026-10-18T10:09:15Z", now)).toBe("45 s");
    expect(elapsed("2026-10-18T09:05:00Z", now)).toBe("1 h 5 min");
    expect(elapsed(null, now)).toBe("");
  });
});
