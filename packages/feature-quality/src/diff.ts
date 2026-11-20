/**
 * A text's change line by line, as a review shows it (decision 0021): the
 * lines kept, taken out and put in, in order (a longest common
 * subsequence), then in hunks with a few kept lines around each change;
 * shown as two copies side by side, what changed inside a line marked.
 */

export type Line = {
  kind: "same" | "del" | "add";
  text: string;
  /** Its number before (kept and taken out) and after (kept and put in). */
  before?: number;
  after?: number;
};

type Op = "same" | "del" | "add";

/** How `b` comes from `a`, item by item (a longest common subsequence):
 * taken out before put in, as a review reads a change. */
function ops<T>(a: T[], b: T[]): Op[] {
  // lcs[i][j]: the longest common run of a[i..] and b[j..].
  const lcs = Array.from({ length: a.length + 1 }, () => new Array<number>(b.length + 1).fill(0));
  for (let i = a.length - 1; i >= 0; i--) {
    for (let j = b.length - 1; j >= 0; j--) {
      lcs[i]![j] =
        a[i] === b[j] ? lcs[i + 1]![j + 1]! + 1 : Math.max(lcs[i + 1]![j]!, lcs[i]![j + 1]!);
    }
  }
  const out: Op[] = [];
  let i = 0;
  let j = 0;
  while (i < a.length || j < b.length) {
    if (i < a.length && j < b.length && a[i] === b[j]) {
      out.push("same");
      i++;
      j++;
    } else if (i < a.length && (j === b.length || lcs[i + 1]![j]! >= lcs[i]![j + 1]!)) {
      out.push("del");
      i++;
    } else {
      out.push("add");
      j++;
    }
  }
  return out;
}

const linesOf = (text: string) => (text === "" ? [] : text.replace(/\n$/, "").split("\n"));

/** The lines of `before` and `after`, each kept, taken out or put in. */
export function lineDiff(before: string, after: string): Line[] {
  const a = linesOf(before);
  const b = linesOf(after);
  let i = 0;
  let j = 0;
  return ops(a, b).map((op) => {
    if (op === "same") return { kind: op, text: a[i]!, before: ++i, after: ++j };
    if (op === "del") return { kind: op, text: a[i]!, before: ++i };
    return { kind: op, text: b[j]!, after: ++j };
  });
}

/** A piece of a line, and whether it is what changed. */
export type Part = { text: string; changed: boolean };

/** What changed inside a line, word by word (spaces and signs their own
 * words): the parts of `before` and of `after`. */
export function inline(before: string, after: string): [Part[], Part[]] {
  const words = (s: string) => s.match(/\w+|\s+|[^\w\s]/g) ?? [];
  const a = words(before);
  const b = words(after);
  const left: Part[] = [];
  const right: Part[] = [];
  const push = (parts: Part[], text: string, changed: boolean) => {
    const last = parts[parts.length - 1];
    if (last && last.changed === changed) last.text += text;
    else parts.push({ text, changed });
  };
  let i = 0;
  let j = 0;
  for (const op of ops(a, b)) {
    if (op !== "add") push(left, a[i++]!, op === "del");
    if (op !== "del") push(right, b[j++]!, op === "add");
  }
  return [left, right];
}

/** A row of the two copies side by side: the line before, the line after. */
export type Row = { left?: Line; right?: Line };

/** A hunk's lines as rows of two copies: a kept line on both sides; a run
 * taken out beside the run put in after it, line by line. */
export function sideBySide(lines: Line[]): Row[] {
  const rows: Row[] = [];
  let k = 0;
  while (k < lines.length) {
    const line = lines[k]!;
    if (line.kind === "same") {
      rows.push({ left: line, right: line });
      k++;
      continue;
    }
    const del: Line[] = [];
    const add: Line[] = [];
    while (k < lines.length && lines[k]!.kind === "del") del.push(lines[k++]!);
    while (k < lines.length && lines[k]!.kind === "add") add.push(lines[k++]!);
    for (let n = 0; n < Math.max(del.length, add.length); n++) {
      rows.push({ left: del[n], right: add[n] });
    }
  }
  return rows;
}

/** The changed lines with `context` kept lines around, in hunks. */
export function hunks(lines: Line[], context = 3): Line[][] {
  const near = lines.map((_, k) =>
    lines.slice(Math.max(0, k - context), k + context + 1).some((l) => l.kind !== "same"),
  );
  const out: Line[][] = [];
  let current: Line[] = [];
  lines.forEach((line, k) => {
    if (near[k]) current.push(line);
    else if (current.length) {
      out.push(current);
      current = [];
    }
  });
  if (current.length) out.push(current);
  return out;
}

/** How many lines were put in and taken out. */
export function counts(lines: Line[]) {
  return {
    added: lines.filter((l) => l.kind === "add").length,
    removed: lines.filter((l) => l.kind === "del").length,
  };
}
