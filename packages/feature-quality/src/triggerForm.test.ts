import { describe, expect, it } from "vitest";
import { newWhen, problemOf, triggerOf, whenOf } from "./triggerForm";

const now = new Date(2026, 10, 4, 9, 0);

describe("triggerOf", () => {
  it("makes the trigger the server reads, or none for a cron", () => {
    const daily = newWhen(now);
    expect(triggerOf(daily)).toEqual({
      start: "2026-11-04T06:00:00",
      recurrence: { kind: "daily", every: 1 },
      repeat: null,
      random_delay_secs: 0,
      expires: null,
    });
    const nth = {
      ...daily,
      kind: "monthly" as const,
      monthMode: "nth" as const,
      nth: [5],
      nthWeekdays: [5],
    };
    expect(triggerOf(nth)?.recurrence).toEqual({
      kind: "monthly",
      months: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12],
      days: [],
      nth: [5],
      weekdays: [5],
    });
    expect(triggerOf({ ...daily, kind: "custom", cron: "0 6 * * *" })).toBeNull();
  });

  it("reads a saved schedule back as it was written", () => {
    const written = {
      ...newWhen(now),
      kind: "weekly" as const,
      everyWeeks: 2,
      weekdays: [1, 4],
      repeatOn: true,
      repeatEvery: 30,
      repeatFor: null,
      delayOn: true,
      delay: 900,
    };
    const trigger = triggerOf(written);
    const back = whenOf({ cron: "", timezone: "America/Lima", trigger });
    expect(triggerOf(back)).toEqual(trigger);
    expect(back.timezone).toBe("America/Lima");
    expect(whenOf({ cron: "0 6 * * 1-5", timezone: "UTC", trigger: null })).toMatchObject({
      kind: "custom",
      cron: "0 6 * * 1-5",
    });
  });
});

describe("problemOf", () => {
  it("says what is missing before saving", () => {
    const w = newWhen(now);
    expect(problemOf(w)).toBeNull();
    expect(problemOf({ ...w, kind: "weekly", weekdays: [] })).not.toBeNull();
    expect(problemOf({ ...w, kind: "custom", cron: " " })).not.toBeNull();
    expect(problemOf({ ...w, repeatOn: true, repeatEvery: 30, repeatFor: 15 })).not.toBeNull();
    expect(problemOf({ ...w, expiresOn: true, expiresDate: "2026-11-01" })).not.toBeNull();
  });
});

describe("newWhen", () => {
  it("is in the platform's time zone, UTC until it is known", () => {
    expect(newWhen(now).timezone).toBe("UTC");
    expect(newWhen(now, "Europe/Madrid").timezone).toBe("Europe/Madrid");
  });
});
