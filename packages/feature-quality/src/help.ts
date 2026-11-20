import { exists, t } from "@knime-orchestador/i18n";
import { ITEM, type Context, type Segment } from "./completion";

/**
 * What a key or a value of a rules file is for, in the person's language
 * (`quality:help.*`), by where it is: a key of the file, of a rule, of its
 * examples, of a reference, of a combination table or its rows, of the
 * placeholders or of an imported rule's origin.
 */
export function scope(path: Segment[]): string | undefined {
  const last = path[path.length - 1];
  if (path.length === 0) return "file";
  if (path.length === 2 && path[0] === "rules" && path[1] === ITEM) return "rule";
  if (last === "examples") return "examples";
  if (last === "origin") return "origin";
  if (path.length === 1 && path[0] === "placeholders") return "placeholders";
  if (path.length === 2 && path[0] === "references") return "reference";
  if (path.length === 2 && path[0] === "tables") return "table";
  if (path.length === 4 && path[0] === "tables" && path[2] === "rows") return "row";
  return undefined;
}

function text(key: string): string | undefined {
  return exists(key) ? t(key) : undefined;
}

/** What a key at `path` is for. */
export function keyHelp(path: Segment[], key: string): string | undefined {
  const where = scope(path);
  return where ? text(`quality:help.keys.${where}.${key}`) : undefined;
}

/** What `value` means for `key` (`check: domain`, `severity: major`…). */
export function valueHelp(key: string, value: string): string | undefined {
  return text(`quality:help.values.${key}.${value}`);
}

/** The help for what `ctx` points at: a key, or a key's value. */
export function help(ctx: Context, label: string): string | undefined {
  return ctx.at === "key" ? keyHelp(ctx.path, label) : valueHelp(ctx.key!, label);
}
