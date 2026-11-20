/**
 * Completion of a rules file from the JSON Schema of its YAML (decision
 * 0019, point 11) and the fields of the dataset's mapping: where the cursor
 * is (a key or a key's value, under which keys), then what may go there.
 * Line by line, by indentation: block YAML as the rules files are written.
 */

/** A key's name, or an item of a list. */
export type Segment = string | typeof ITEM;
export const ITEM = 0;

export type Context = {
  /** The keys and list items above the cursor, from the top. */
  path: Segment[];
  /** A key to write, or the value of `key`. */
  at: "key" | "value";
  key?: string;
  /** The keys already beside it (not offered again). */
  siblings: string[];
  /** Where the word being typed starts, and the word. */
  from: number;
  word: string;
};

export type Suggestion = { label: string; kind: "key" | "value" | "field"; info?: string };

type Line = { indent: number; dash: number; key?: string };

const LINE = /^(\s*)(-\s+)?(?:([A-Za-z_][\w-]*)\s*:(?=\s|$))?/;

function parse(line: string): Line | undefined {
  if (/^\s*(#.*)?$/.test(line)) return undefined;
  const m = LINE.exec(line)!;
  return { indent: m[1]!.length, dash: m[2]?.length ?? 0, key: m[3] };
}

const content = (l: Line) => l.indent + l.dash;

/** The keys and items that hold a line, walking up from line `i`. */
function parents(lines: (Line | undefined)[], i: number, bound: number, item: boolean) {
  const path: Segment[] = item ? [ITEM] : [];
  for (; i >= 0 && (bound > 0 || item); i--) {
    const l = lines[i];
    if (!l) continue;
    if (item) {
      // Looking for the key that owns a list whose dash is at `bound`.
      if (l.indent > bound || (l.dash && l.indent === bound) || !l.key) continue;
      path.push(l.key);
      if (l.dash) path.push(ITEM);
      bound = l.indent;
      item = l.dash > 0;
    } else if (l.dash && content(l) === bound) {
      path.push(ITEM);
      bound = l.indent;
      item = true;
    } else if (content(l) < bound) {
      if (l.key) path.push(l.key);
      if (l.dash) path.push(ITEM);
      bound = l.indent;
      item = l.dash > 0;
    }
  }
  return path.reverse();
}

/** The keys at `indent` of the block line `i` is in. */
function siblings(lines: (Line | undefined)[], i: number, indent: number) {
  const keys: string[] = [];
  const own = lines[i];
  // Up, unless the line starts a list item: to the item's dash or the parent.
  for (let j = i - 1; j >= 0 && !(own?.dash && content(own) === indent); j--) {
    const l = lines[j];
    if (!l || content(l) > indent) continue;
    if (content(l) < indent) break;
    if (l.key) keys.push(l.key);
    if (l.dash) break;
  }
  for (let j = i + 1; j < lines.length; j++) {
    const l = lines[j];
    if (!l || content(l) > indent) continue;
    if (content(l) < indent || l.dash) break;
    if (l.key) keys.push(l.key);
  }
  return keys;
}

/** Whether line `i`, indented `indent`, is inside a block value (`>-`, `|`). */
function inBlockValue(lines: string[], i: number, indent: number): boolean {
  for (let j = i - 1; j >= 0; j--) {
    const line = lines[j]!;
    if (!line.trim()) continue;
    if (line.length - line.trimStart().length < indent) {
      return /:\s*[>|][-+]?\d*\s*(#.*)?$/.test(line);
    }
  }
  return false;
}

/** Where `pos` is in `text`, or `undefined` where nothing is completed. */
export function context(text: string, pos: number): Context | undefined {
  const start = text.lastIndexOf("\n", pos - 1) + 1;
  const prefix = text.slice(start, pos);
  const lines = text.split("\n").map(parse);
  const i = text.slice(0, start).split("\n").length - 1;
  const asKey = /^(\s*)(-\s+)?([\w-]*)$/.exec(prefix);
  const asValue = /^(\s*)(-\s+)?([A-Za-z_][\w-]*)\s*:\s+["']?([\w-]*)$/.exec(prefix);
  const m = asKey ?? asValue;
  if (!m || inBlockValue(text.split("\n"), i, m[1]!.length)) return undefined;
  const indent = m[1]!.length;
  const dash = m[2]?.length ?? 0;
  const word = (asKey ? m[3] : m[4]) ?? "";
  const path = dash ? parents(lines, i - 1, indent, true) : parents(lines, i - 1, indent, false);
  const others = siblings(lines, i, indent + dash).filter((k) => k !== m[3]);
  return {
    path,
    at: asKey ? "key" : "value",
    key: asKey ? undefined : m[3],
    siblings: others,
    from: pos - word.length,
    word,
  };
}

type Node = Record<string, unknown>;

/** A node's alternatives, references followed and `null` dropped. */
function resolve(root: Node, node: unknown, depth = 0): Node[] {
  if (!node || typeof node !== "object" || depth > 16) return [];
  const n = node as Node;
  if (typeof n.$ref === "string") {
    const name = n.$ref.replace("#/$defs/", "");
    return resolve(root, (root.$defs as Node | undefined)?.[name], depth + 1);
  }
  const alternatives = (n.anyOf ?? n.oneOf) as unknown[] | undefined;
  if (alternatives) return alternatives.flatMap((a) => resolve(root, a, depth + 1));
  return n.type === "null" ? [] : [n];
}

function child(root: Node, nodes: Node[], segment: Segment): Node[] {
  return nodes.flatMap((n) => {
    if (segment === ITEM) return resolve(root, n.items);
    const props = n.properties as Node | undefined;
    const extra = typeof n.additionalProperties === "object" ? n.additionalProperties : undefined;
    return resolve(root, props?.[segment] ?? extra);
  });
}

/** Keys of a rule that name a field of the mapping. */
const FIELD_KEYS = new Set(["field", "key", "from"]);

/** What may go at `ctx`, by `schema`; a rule's fields from `fields`. */
export function suggest(schema: Node, ctx: Context, fields: string[]): Suggestion[] {
  const nodes = ctx.path.reduce((at, s) => child(schema, at, s), resolve(schema, schema));
  const word = ctx.word.toLowerCase();
  const starts = (s: Suggestion) => s.label.toLowerCase().startsWith(word);
  if (ctx.at === "key") {
    const seen = new Set<string>();
    const keys = nodes.flatMap((n) =>
      Object.entries((n.properties as Record<string, Node> | undefined) ?? {}).map(
        ([label, p]) => ({
          label,
          kind: "key" as const,
          info: typeof p.description === "string" ? p.description : undefined,
        }),
      ),
    );
    return keys
      .filter((k) => !ctx.siblings.includes(k.label) && !seen.has(k.label) && seen.add(k.label))
      .filter(starts);
  }
  const isRule = ctx.path.length === 2 && ctx.path[0] === "rules" && ctx.path[1] === ITEM;
  if (isRule && FIELD_KEYS.has(ctx.key!)) {
    return fields.map((label) => ({ label, kind: "field" as const })).filter(starts);
  }
  const values = child(schema, nodes, ctx.key!).flatMap((n) => {
    if (Array.isArray(n.enum)) return n.enum.map(String);
    return n.type === "boolean" || (Array.isArray(n.type) && n.type.includes("boolean"))
      ? ["true", "false"]
      : [];
  });
  return [...new Set(values)].map((label) => ({ label, kind: "value" as const })).filter(starts);
}
