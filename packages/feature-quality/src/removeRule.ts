/**
 * A rules file without one of its rules (the tree's _Delete rule_,
 * decision 0051): the list item whose `id` is it, taken out with its lines,
 * the rest of the text as it was. `undefined` if no item has that id (a
 * rule a list gives several of, `R-01.Field`, is not one).
 */
export function withoutRule(text: string, id: string): string | undefined {
  // The file's last newline is no line of the last rule.
  const newline = text.endsWith("\n");
  const lines = (newline ? text.slice(0, -1) : text).split("\n");
  const indent = (l: string) => /^\s*/.exec(l)?.[0].length ?? 0;
  const blank = (l: string) => /^\s*(#.*)?$/.test(l);
  const isId = (l: string) =>
    /^\s*(?:-\s+)?id:\s*["']?([^"'\s#]+)["']?\s*(?:#.*)?$/.exec(l)?.[1] === id;
  const at = lines.findIndex(isId);
  if (at < 0) return undefined;
  // Its item: the `-` at or above the id, at the list's indent.
  let start = at;
  while (start >= 0 && !/^\s*-(\s|$)/.test(lines[start])) start--;
  if (start < 0) return undefined;
  const item = indent(lines[start]);
  let end = start + 1;
  while (end < lines.length) {
    const l = lines[end];
    if (!blank(l) && (indent(l) < item || (indent(l) === item && /^\s*-(\s|$)/.test(l)))) break;
    if (!blank(l) && indent(l) === item && !/^\s*-/.test(l)) break;
    end++;
  }
  // A blank line after it goes with it; the next item keeps its own.
  const rest = [...lines.slice(0, start), ...lines.slice(end)];
  const top = rest.findIndex((l) => /^rules:\s*(#.*)?$/.test(l));
  const left = rest.some((l, i) => i > top && /^\s*-(\s|$)/.test(l));
  if (top >= 0 && !left) rest[top] = "rules: []";
  return rest.join("\n") + (newline ? "\n" : "");
}
