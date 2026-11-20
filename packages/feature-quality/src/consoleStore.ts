import { createSignal } from "solid-js";
import { consoleApi, type ConsoleOut } from "./api";
import { masked, reveal, scopes, type Reason, type Revealing } from "./reveal";
import { revealApi } from "./revealApi";

/** How often a statement running is asked after. */
const POLL_MS = 1000;
/** How many statements are kept. */
const KEPT = 20;

/** What the console starts with: the lake's tables. */
export const FIRST =
  "SELECT\n  schema_name,\n  table_name\nFROM\n  duckdb_tables()\nWHERE\n  database_name = 'lake'\nORDER BY\n  1,\n  2";

/** Where a statement is: being run, or what came of it. */
export type Ran =
  { phase: "running" } | ({ phase: "done" } & Revealing<ConsoleOut>) | { phase: "refused" };

/** A statement run, with what came of it. */
export interface Asked {
  id: number;
  sql: string;
  at: Date;
  ran: Ran;
  /** The columns of its result shown, in order; all of them if none. */
  columns?: string[];
}

/**
 * The console's memory (decision 0054), while the page is open: the
 * statement being written, the last ones run with their results, and which
 * is shown. Closed and opened again, the console is as it was; a statement
 * still running is followed meanwhile. Never in the browser's storage: a
 * result holds values.
 */
const [draft, setDraft] = createSignal(FIRST);
const [asked, setAsked] = createSignal<Asked[]>([]);
const [shown, setShown] = createSignal<number>();
export { asked, draft, setDraft, shown };

let next = 1;

const update = (id: number, ran: Ran) =>
  setAsked((all) => all.map((a) => (a.id === id ? { ...a, ran } : a)));

async function follow(id: number, job: string) {
  try {
    const out = await consoleApi.read(job);
    if (out.state === "done" || out.state === "failed") {
      update(id, { phase: "done", job, masked: out, out });
      return showWhole(id);
    }
  } catch {
    return update(id, { phase: "refused" });
  }
  setTimeout(() => void follow(id, job), POLL_MS);
}

const doneOf = (id: number) => {
  const ran = asked().find((a) => a.id === id)?.ran;
  return ran?.phase === "done" ? ran : undefined;
};

/**
 * Shows statement `id`'s result whole (decision 0069) with `reason`, or
 * with the reason kept for the lake: one post, when its read says it may.
 */
export async function showWhole(id: number, reason?: Reason) {
  const ran = doneOf(id);
  if (!ran) return;
  const post = (r: Reason) => revealApi.console(ran.job, r);
  await reveal(scopes.lake, ran, post, (v) => update(id, { phase: "done", ...v }), reason);
}

/** Shows statement `id`'s result masked again. */
export function maskAgain(id: number) {
  const ran = doneOf(id);
  if (ran) update(id, { phase: "done", ...masked(ran) });
}

/** Runs `sql`, kept first among the statements and shown. */
export async function runStatement(sql: string) {
  const id = next++;
  setAsked((all) =>
    [{ id, sql, at: new Date(), ran: { phase: "running" } as Ran }, ...all].slice(0, KEPT),
  );
  setShown(id);
  try {
    const { job } = await consoleApi.run(sql);
    setTimeout(() => void follow(id, job), POLL_MS);
  } catch {
    update(id, { phase: "refused" });
  }
}

/** A statement kept, shown again: its text back in the editor, its result below. */
export function showStatement(id: number) {
  const a = asked().find((x) => x.id === id);
  if (!a) return;
  setShown(id);
  setDraft(a.sql);
}

/** A statement forgotten. */
export function forget(id: number) {
  setAsked((all) => all.filter((a) => a.id !== id));
  if (shown() === id) setShown(undefined);
}

/** The statement shown, if any. */
export const current = () => asked().find((a) => a.id === shown());

/** The columns shown of a statement's result; `undefined` for all of them. */
export const columnsShown = (id: number) => asked().find((a) => a.id === id)?.columns;

/** Which columns of a statement's result are shown: `undefined` for all. */
export const showColumns = (id: number, columns?: string[]) =>
  setAsked((all) => all.map((a) => (a.id === id ? { ...a, columns } : a)));
