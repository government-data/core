import { describe, expect, it } from "vitest";
import { describeTrigger, span, type ScheduleTrigger } from "./scheduleTrigger";

const base = (recurrence: ScheduleTrigger["recurrence"]): ScheduleTrigger => ({
  start: "2026-07-09T09:27:00",
  recurrence,
  random_delay_secs: 0,
});

describe("describeTrigger", () => {
  it("writes each recurrence in Spanish", () => {
    expect(describeTrigger(base({ kind: "once" }))).toBe("Una vez, el 9 jul 2026 a las 09:27");
    expect(describeTrigger(base({ kind: "daily", every: 2 }))).toBe(
      "Cada 2 días a las 09:27 · desde el 9 jul 2026",
    );
    expect(describeTrigger(base({ kind: "weekly", every: 2, days: [4, 1] }))).toBe(
      "Cada 2 semanas los lun y jue a las 09:27 · desde el 9 jul 2026",
    );
    expect(
      describeTrigger(
        base({
          kind: "monthly",
          months: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12],
          days: [32],
          nth: [],
          weekdays: [],
        }),
      ),
    ).toBe("El último día de todos los meses a las 09:27 · desde el 9 jul 2026");
    expect(
      describeTrigger(base({ kind: "monthly", months: [6, 3], days: [], nth: [1], weekdays: [1] })),
    ).toBe("El primer lun de mar y jun a las 09:27 · desde el 9 jul 2026");
  });

  it("adds repetition, random delay and expiration", () => {
    const t: ScheduleTrigger = {
      ...base({ kind: "daily", every: 1 }),
      repeat: { every_minutes: 15, for_minutes: 60 },
      random_delay_secs: 300,
      expires: "2027-10-17T17:39:00",
    };
    expect(describeTrigger(t)).toBe(
      "Todos los días a las 09:27 · desde el 9 jul 2026 · repite cada 15 min durante 1 h · retraso al azar hasta 5 min · expira el 17 oct 2027 a las 17:39",
    );
  });
});

it("span writes short durations", () => {
  expect([span(30), span(900), span(5400), span(86400), span(259200)]).toEqual([
    "30 s",
    "15 min",
    "1.5 h",
    "1 día",
    "3 días",
  ]);
});
