/**
 * _Test_ in the editor (decision 0043, point 3): the rule at the cursor
 * found in the text, a test queued and waited for, and its records read a
 * page at a time.
 */

import { createSignal, onCleanup } from "solid-js";
import { QualityError, rulesApi, type RuleTested, type TestedRecord, type TestOut } from "./api";
import { masked, reveal, scopes, type Reason, type Revealing } from "./reveal";
import { revealApi } from "./revealApi";

type TestReport = NonNullable<TestOut["report"]>;

const indent = (line: string) => line.length - line.trimStart().length;
const blank = (line: string) => line.trim() === "" || line.trimStart().startsWith("#");

/**
 * The id of the rule whose item holds `line` (from 1), under the file's
 * top-level `rules:`; none outside a rule.
 */
export function ruleAt(text: string, line: number): string | undefined {
  const lines = text.split("\n");
  const top = lines.findIndex((l) => /^rules:\s*(#.*)?$/.test(l));
  if (top < 0 || line - 1 <= top) return undefined;
  const first = lines.findIndex((l, i) => i > top && /^\s*-(\s|$)/.test(l));
  if (first < 0) return undefined;
  const item = indent(lines[first]);
  const isItem = (l: string) => indent(l) === item && /^\s*-(\s|$)/.test(l);
  let start = Math.min(line - 1, lines.length - 1);
  while (start > top && !isItem(lines[start])) {
    if (!blank(lines[start]) && indent(lines[start]) < item) return undefined;
    start--;
  }
  if (start <= top) return undefined;
  // The item's own keys sit where its first one does, after `- `.
  const keys = /^\s*-\s+/.exec(lines[start])?.[0].length ?? item + 2;
  for (let i = start; i < lines.length; i++) {
    const l = lines[i];
    if (i > start && !blank(l) && (indent(l) <= item || isItem(l))) break;
    const m = /^\s*(?:-\s+)?id:\s*["']?([^"'\s#]+)["']?\s*(?:#.*)?$/.exec(l);
    if (m && (i === start || indent(l) === keys)) return m[1];
  }
  return undefined;
}

/** Of the records that apply, the share that passes, in percent; none if none applies. */
export function conformity(r: Pick<RuleTested, "applies" | "passes">): number | undefined {
  return r.applies > 0 ? Math.round((r.passes / r.applies) * 1000) / 10 : undefined;
}

/** The fields of a read of records, in their order: the key's first. */
export function columns(records: TestedRecord[]): string[] {
  const seen: string[] = [];
  for (const r of records)
    for (const v of r.values) if (!seen.includes(v.field)) seen.push(v.field);
  return seen;
}

/** Whether a test's report lists any record, under a rule or one of its groups. */
export const hasValues = (report: Pick<TestReport, "rules">) =>
  report.rules.some((r) => r.records.length > 0 || (r.groups ?? []).some((g) => g.records.length));

/** Records shown a page at a time. */
export const PAGE = 10;

/** The page `n` (from 0) of `records`. */
export const page = <T>(records: T[], n: number): T[] => records.slice(n * PAGE, (n + 1) * PAGE);

/** How often a test is asked after while it runs. */
const POLL_MS = 1500;

/** A test as the editor shows it. */
export type TestState =
  | { phase: "idle" }
  | { phase: "waiting"; rule?: string; cut?: string; state: string; since: number }
  | ({ phase: "done"; rule?: string } & Revealing<TestOut>)
  | { phase: "expired"; rule?: string }
  | { phase: "refused"; rule?: string; reason: "lint" | "no_cut" | "forbidden" | "other" };

/** Why the API refused a test, as the editor names it. */
function reason(e: unknown): "lint" | "no_cut" | "forbidden" | "other" {
  if (!(e instanceof QualityError)) return "other";
  const error = (e.body as { error?: string } | undefined)?.error;
  if (e.status === 422 && error === "lint") return "lint";
  if (e.status === 409 && error === "no_cut") return "no_cut";
  return e.status === 403 ? "forbidden" : "other";
}

/** A test asked for, enough to ask after it again. */
interface Asked {
  job: string;
  rule?: string;
  cut?: string;
  since: number;
}

/** Each rule's last test while the app is open, by `dataset/rule`: its counts only. */
const TESTED = new Map<string, Pick<RuleTested, "applies" | "fails">>();
// What reads them follows each test that ends (the explorer stays open meanwhile).
const [tests, setTests] = createSignal(0);

/** How `rule` of `dataset` came out of its last test in this app, if it was tested. */
export const lastTested = (dataset: string, rule: string) => {
  tests();
  return TESTED.get(`${dataset}/${rule}`);
};

/** Each file's last test while the app is open, by `dataset/file`. */
const LAST = new Map<string, TestState>();
const STORED = "qd.editor.test.";

/** The last test asked on a file, kept in the tab: its job only, never a record. */
const stored = {
  read: (key: string): Asked | undefined => {
    try {
      return JSON.parse(sessionStorage.getItem(STORED + key) ?? "null") ?? undefined;
    } catch {
      return undefined;
    }
  },
  write: (key: string, asked: Asked) => {
    try {
      sessionStorage.setItem(STORED + key, JSON.stringify(asked));
    } catch {
      // Without storage, a reload forgets the test: nothing else does.
    }
  },
  forget: (key: string) => {
    try {
      sessionStorage.removeItem(STORED + key);
    } catch {
      // Without storage, nothing was kept.
    }
  },
};

/**
 * Tests of the text as typed on `dataset`'s latest cut: `run` queues one
 * and asks after it until it ends; a newer test drops an older one. The
 * last test of `file` is kept: in memory while the app is open, and its
 * job in the tab, asked after again on a reload. Call within a component:
 * its timer ends with it.
 */
export function createRuleTest(dataset: () => string, file: () => string) {
  const key = () => `${dataset()}/${file()}`;
  const [test, setShown] = createSignal<TestState>(LAST.get(key()) ?? { phase: "idle" });
  const setTest = (x: TestState) => {
    LAST.set(key(), x);
    setShown(x);
  };
  let timer: ReturnType<typeof setTimeout> | undefined;
  let asked = 0;
  onCleanup(() => clearTimeout(timer));

  const follow = async (mine: number, a: Asked) => {
    if (mine !== asked) return;
    try {
      const out = await rulesApi.readTest(a.job);
      if (mine !== asked) return;
      // Done, its report purged after a day: nothing left to ask after.
      if (out.state === "expired") {
        stored.forget(key());
        setTest({ phase: "expired", rule: a.rule });
        return;
      }
      if (out.state === "done" || out.state === "failed") {
        for (const r of out.report?.rules ?? [])
          TESTED.set(`${dataset()}/${r.rule}`, { applies: r.applies, fails: r.fails });
        setTests((n) => n + 1);
        setTest({ phase: "done", rule: a.rule, job: a.job, masked: out, out });
        return showWhole(mine);
      }
      setTest({ phase: "waiting", rule: a.rule, cut: a.cut, state: out.state, since: a.since });
    } catch (e) {
      if (mine === asked) setTest({ phase: "refused", rule: a.rule, reason: reason(e) });
      return;
    }
    timer = setTimeout(() => void follow(mine, a), POLL_MS);
  };

  // Shown whole with the reason kept for the dataset, or with `reason` (decision 0069).
  const showWhole = async (mine: number, reason?: Reason) => {
    const x = test();
    if (x.phase !== "done") return;
    const post = (r: Reason) => revealApi.test(x.job, r);
    await reveal(
      scopes.dataset(dataset()),
      x,
      post,
      (v) => {
        if (mine === asked) setTest({ ...x, ...v });
      },
      reason,
    );
  };

  // A test still on its way, or one this tab asked before a reload: asked after again.
  const before = stored.read(key());
  const resumed = !!before && (test().phase === "waiting" || test().phase === "idle");
  if (before && resumed) void follow(++asked, before);

  return {
    test,
    /** Whether there is a test of this file to show, or one being asked after again. */
    known: test().phase !== "idle" || resumed,
    running: () => test().phase === "waiting",
    /** Shows the test's values whole with `reason`. */
    showWhole: (reason: Reason) => void showWhole(asked, reason),
    /** Shows them masked again. */
    mask: () => {
      const x = test();
      if (x.phase === "done") setTest({ ...x, ...masked(x) });
    },
    /** Tests `yaml` (only `rule`, if given), its records with `columns` too. */
    run: async (yaml: string, rule?: string, columns: string[] = []) => {
      clearTimeout(timer);
      const mine = ++asked;
      const since = Date.now();
      setTest({ phase: "waiting", rule, state: "queued", since });
      try {
        const started = await rulesApi.test(dataset(), yaml, rule, columns);
        const a = { job: started.job, rule, cut: started.cut, since };
        stored.write(key(), a);
        await follow(mine, a);
      } catch (e) {
        if (mine === asked) setTest({ phase: "refused", rule, reason: reason(e) });
      }
    },
  };
}
