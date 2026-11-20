/** A rules file with a rule added for a field, and the line it starts on (1-based). */
export interface WithNewRule {
  yaml: string;
  id: string;
  line: number;
}

const indent = (line: string) => line.length - line.trimStart().length;
const blank = (line: string) => !line.trim() || line.trimStart().startsWith("#");

/** An id no rule of the file has yet: `<field>-01`, `<field>-02`… */
export function freeId(yaml: string, field: string): string {
  const taken = new Set([...yaml.matchAll(/^\s*-?\s*id:\s*["']?([^"'\s#]+)/gm)].map((m) => m[1]));
  for (let n = 1; ; n++) {
    const id = `${field}-${String(n).padStart(2, "0")}`;
    if (!taken.has(id)) return id;
  }
}

/**
 * The file with a rule for `field` at the end of its `rules:` list, its
 * pattern `required` for the person to change: the editor opens there and
 * checks it as typed, nothing saved until they save.
 */
export function withNewRule(yaml: string, field: string): WithNewRule {
  const id = freeId(yaml, field);
  const lines = yaml.replace(/\n+$/, "").split("\n");
  let at = lines.findIndex((l) => /^rules:\s*(\[\s*\])?\s*(#.*)?$/.test(l));
  if (at < 0) {
    lines.push("rules:");
    at = lines.length - 1;
  } else {
    lines[at] = "rules:";
  }
  const first = lines.slice(at + 1).find((l) => !blank(l));
  const pad = first?.trimStart().startsWith("-") ? " ".repeat(indent(first)) : "  ";
  let end = at + 1;
  for (let i = at + 1; i < lines.length; i++) {
    const l = lines[i];
    if (blank(l)) continue;
    if (indent(l) === 0 && !(pad === "" && l.startsWith("-"))) break;
    end = i + 1;
  }
  const rule = [`${pad}- id: ${id}`, `${pad}  field: ${field}`, `${pad}  check: required`];
  lines.splice(end, 0, ...rule);
  return { yaml: lines.join("\n") + "\n", id, line: end + 1 };
}

/** A field's own rules file (decision 0044), if its name can be one. */
export function fieldFile(field: string): string | undefined {
  return /^[A-Za-z0-9_-]{1,64}$/.test(field) ? `${field}.yaml` : undefined;
}

/** The table's global rules (decision 0045): declarations and named rules, no rules. */
export const GLOBAL_FILE = "reglas_globales.yaml";

/**
 * A rules file `main` does not have yet, as it starts: the dataset and no
 * rules; the global rules, the dataset alone.
 */
export const emptyFile = (dataset: string, file?: string) =>
  file === GLOBAL_FILE ? `dataset: ${dataset}\n` : `dataset: ${dataset}\nrules: []\n`;
