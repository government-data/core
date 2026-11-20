import { describe, expect, it } from "vitest";
import { cutState, describeCron, outcome, scheduleBody } from "./schedules";

describe("describeCron", () => {
  it("says the common ones in words and leaves the rest as written", () => {
    expect(describeCron("0 6 * * *")).toBe("Todos los días a las 06:00");
    expect(describeCron("30 7 * * 1-5")).toBe("De lunes a viernes a las 07:30");
    expect(describeCron("0 2 * * 0")).toBe("Cada domingo a las 02:00");
    expect(describeCron("0 5 1 * *")).toBe("El día 1 de cada mes a las 05:00");
    expect(describeCron("*/15 * * * *")).toBe("*/15 * * * *");
    expect(describeCron("0 6 * *")).toBe("0 6 * *");
  });
});

describe("states", () => {
  it("gives each cut state and outcome a tone and a label, never color alone", () => {
    expect(cutState("done")).toEqual({ tone: "good", label: "Evaluado" });
    expect(cutState("failed").tone).toBe("critical");
    expect(cutState("something_new").tone).toBe("neutral");
    expect(outcome("blocked").tone).toBe("serious");
    expect(outcome(null).label).not.toBe("");
  });
});

describe("scheduleBody", () => {
  it("sends a cut's fields or a job's, never both", () => {
    const form = {
      cron: " 0 6 * * * ",
      timezone: "America/Bogota",
      enabled: true,
      dataset: "loans",
      rerun: "replace",
      kind: "maintain",
    } as const;
    expect(scheduleBody({ ...form, fires: "cut" })).toEqual({
      cron: "0 6 * * *",
      timezone: "America/Bogota",
      enabled: true,
      dataset: "loans",
      fires: "cut",
      rerun: "replace",
    });
    expect(scheduleBody({ ...form, fires: "job" })).toMatchObject({
      fires: "job",
      kind: "maintain",
    });
    expect(scheduleBody({ ...form, fires: "job" })).not.toHaveProperty("rerun");
  });

  it("sends a trigger instead of the cron when there is one", () => {
    const trigger = {
      start: "2026-11-05T06:00:00",
      recurrence: { kind: "once" as const },
      random_delay_secs: 0,
    };
    const body = scheduleBody({
      cron: "0 6 * * *",
      trigger,
      timezone: "America/Bogota",
      enabled: true,
      dataset: "loans",
      fires: "cut",
      rerun: "block",
      kind: "maintain",
    });
    expect(body).toMatchObject({ cron: "", trigger });
  });
});
