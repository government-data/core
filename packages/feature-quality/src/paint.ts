/**
 * The colours of a rules file by what each word is (decision 0019), above
 * the YAML's own: the dataset's fields, a pattern's allowed values, and the
 * expressions in SQL with their keywords, functions, texts, numbers and
 * `:cut`. Line by line, as the rules files are written; it only colours,
 * lint says what is wrong.
 */

export type Kind =
  "field" | "value" | "expr" | "keyword" | "function" | "text" | "number" | "param";
export type Paint = { from: number; to: number; kind: Kind };

/** Keys whose value is an expression (`min` and `max` may be one; a
 * table's `when` and `then` name fields instead). */
const EXPRESSIONS = new Set(["fails", "if", "then", "when", "where", "min", "max"]);
/** Keys of a rule whose value names fields. */
const FIELDS = new Set(["field", "key", "from"]);
/** Keys of a rule whose value is one of a set (the JSON Schema's enums). */
const VALUES = new Set(["check", "severity", "cause", "dimension", "case", "on_empty", "named"]);
/** Top-level sections whose entries are expressions. */
const EXPRESSION_SECTIONS = new Set(["derived", "populations"]);
const EXAMPLE_KEYS = new Set(["fails", "passes", "skips"]);
const KEYWORDS = new Set(
  (
    "and or not in is null like ilike between case when then else end true false " +
    "exists distinct cast as"
  ).split(" "),
);

/** `key: value` pairs of a line, block or flow (`{ a: 1, b: "x" }`). */
const PAIR =
  /([A-Za-z_][\w-]*)\s*:(?:\s+|$|(?=["'[{]))("(?:[^"\\]|\\.)*"|'(?:[^']|'')*'|[^,{}#\n]*)/g;
/** The header of a block value: folded or literal, with its chomping. */
const BLOCK_HEADER = /^[>|][-+]?\d*$/;
const TOKEN = /'(?:[^']|'')*'|:[a-z_]+|\d+(?:\.\d+)?|[A-Za-z_]\w*|\S/g;

/** An expression's words, from `at` in the document. */
function expression(body: string, at: number, known: Set<string>): Paint[] {
  const out: Paint[] = [{ from: at, to: at + body.length, kind: "expr" }];
  for (const m of body.matchAll(TOKEN)) {
    const word = m[0];
    const from = at + m.index;
    const to = from + word.length;
    const after = body.slice(m.index + word.length).trimStart();
    let kind: Kind | undefined;
    if (word.startsWith("'")) kind = "text";
    else if (word.startsWith(":") && word.length > 1) kind = "param";
    else if (/^\d/.test(word)) kind = "number";
    else if (KEYWORDS.has(word.toLowerCase())) kind = "keyword";
    else if (/^[A-Za-z_]/.test(word) && after.startsWith("(")) kind = "function";
    else if (known.has(word)) kind = "field";
    if (kind) out.push({ from, to, kind });
  }
  return out;
}

/** A value without its quotes, and where it starts. */
function unquote(value: string, at: number): [string, number] {
  const quoted = /^(["']).*\1$/s.test(value) && value.length >= 2;
  return quoted ? [value.slice(1, -1), at + 1] : [value.trimEnd(), at];
}

/** The names `derived:` declares, fields like the mapping's. */
function derived(lines: string[]): string[] {
  const names: string[] = [];
  let inside = false;
  for (const line of lines) {
    if (/^\S/.test(line)) inside = /^derived\s*:/.test(line);
    const m = inside ? /^\s+([A-Za-z_]\w*)\s*:/.exec(line) : null;
    if (m) names.push(m[1]!);
  }
  return names;
}

/** What to colour in `text`, a rules file of a dataset with `fields`. */
export function paint(text: string, fields: string[]): Paint[] {
  const lines = text.split("\n");
  const known = new Set([...fields, ...derived(lines)]);
  const out: Paint[] = [];
  let section = "";
  let examples = -1; // the indent of `examples:` while inside it
  // A block value (`fails: >-`) under way: its key's indent, and whether
  // its lines are an expression.
  let scalar: { indent: number; expression: boolean } | undefined;
  let at = 0;
  for (const line of lines) {
    const spaces = line.length - line.trimStart().length;
    if (scalar && (!line.trim() || spaces > scalar.indent)) {
      if (scalar.expression && line.trim()) {
        out.push(...expression(line.trimEnd().slice(spaces), at + spaces, known));
      }
      at += line.length + 1;
      continue;
    }
    scalar = undefined;
    const indent = /^\s*(-\s+)?/.exec(line)![0].length;
    if (/^[A-Za-z_]/.test(line)) section = /^([\w-]+)/.exec(line)![1]!;
    if (examples >= 0 && line.trim() && indent <= examples) examples = -1;
    for (const m of line.matchAll(PAIR)) {
      const key = m[1]!;
      const valueStart = m.index + m[0].length - m[2]!.length;
      // A block value, plain, runs to the end of its line (its commas too),
      // short of a comment; a flow one stops at `,` or `}`.
      const block = m.index === indent && !/^["'[{]/.test(line.slice(valueStart));
      const value = block ? line.slice(valueStart).split(/\s#/)[0]!.trimEnd() : m[2]!;
      const keyAt = at + m.index;
      const valueAt = at + valueStart;
      const entry = indent > 0 && m.index === indent;
      if (key === "examples") examples = indent;
      if ((examples >= 0 && !EXAMPLE_KEYS.has(key)) || (section === "derived" && entry)) {
        if (known.has(key)) out.push({ from: keyAt, to: keyAt + key.length, kind: "field" });
      }
      if (!value.trim() || examples >= 0) continue;
      const [body, bodyAt] = unquote(value, valueAt);
      const isExpression =
        (EXPRESSION_SECTIONS.has(section) && entry) ||
        (EXPRESSIONS.has(key) && section !== "tables");
      if (BLOCK_HEADER.test(body)) scalar = { indent: spaces, expression: isExpression };
      else if (isExpression) out.push(...expression(body, bodyAt, known));
      else if (
        (section === "rules" && FIELDS.has(key)) ||
        (section === "tables" && /^(when|then)$/.test(key))
      ) {
        for (const w of body.matchAll(/[A-Za-z_]\w*/g)) {
          if (known.has(w[0]))
            out.push({ from: bodyAt + w.index, to: bodyAt + w.index + w[0].length, kind: "field" });
        }
      } else if (section === "rules" && VALUES.has(key)) {
        out.push({ from: bodyAt, to: bodyAt + body.length, kind: "value" });
      }
      if (block) break; // the rest of the line was its value
    }
    at += line.length + 1;
  }
  return out;
}
