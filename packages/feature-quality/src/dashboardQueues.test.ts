import { describe, expect, it } from "vitest";
import { chartFigure, chartHtml } from "./dashboard";

// Keys as texts, so the test reads what each place asks for.
const tr = (key: string) => `[${key}]`;

// Synthetic offices and an advisor.
const rows = [
  {
    dimension: "office",
    grp: "O-2",
    open: 30,
    overdue: 12,
    resolved: 4,
    oldest_days: 90,
    sla_days: 30,
    priority: 6.25,
  },
  {
    dimension: "office",
    grp: "",
    open: 5,
    overdue: 0,
    resolved: 0,
    oldest_days: null,
    sla_days: 30,
    priority: 1,
  },
  {
    dimension: "advisor",
    grp: "A-1",
    open: 2,
    overdue: 2,
    resolved: 1,
    oldest_days: 59,
    sla_days: 15,
    priority: 0.4,
  },
];

describe("the remediation queues", () => {
  it("draws each queue's open and overdue findings, the rest in its table", () => {
    const f = chartFigure("remediation_queues", rows.slice(0, 2), "en", tr)!;
    expect(f.title).toBe("[remediation_queues]");
    expect(f.plot.legend?.map((l) => l.label)).toEqual(["[openFindings]", "[overdueFindings]"]);
    expect(f.plot.table.head).toEqual([
      "[group]",
      "[openFindings]",
      "[overdueFindings]",
      "[resolvedFindings]",
      "[oldestOpen]",
      "[sla]",
      "[priority]",
    ]);
    expect(f.plot.table.rows).toEqual([
      ["O-2", "30", "12", "4", "90 [days]", "30 [days]", "6.25"],
      ["[emptyGroup]", "5", "0", "0", "—", "30 [days]", "1.00"],
    ]);
  });

  it("is drawn once per dimension, named after it", () => {
    const html = chartHtml("remediation_queues", rows, "en", tr);
    expect(html).toContain("[remediation_queues] · office");
    expect(html).toContain("[remediation_queues] · advisor");
    expect(chartFigure("remediation_queues", [], "en", tr)).toBeUndefined();
  });
});
