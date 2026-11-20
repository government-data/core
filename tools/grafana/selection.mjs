// Which rows each panel of a selection shows (check-queries.mjs): a panel
// repeated by a variable, one per selected value, and a panel following a
// variable's selection by Grafana 13.2.5's regex value matcher. Pure; the
// values of a variable are { values, every }, `every` at All.

/** The values of a frames' column, as text. */
const column = (frames, name) =>
  frames.flatMap((f) => {
    const at = f.schema.fields.findIndex((x) => x.name === name);
    return at < 0 ? [] : (f.data.values[at] ?? []).map((v) => (v == null ? v : String(v)));
  });

/**
 * A repeated panel's rows by its repeat variable's selected values, as
 * Grafana draws a panel per value and keeps the rows its filterByValue
 * matches: at All, fails when a row is in no panel.
 */
export function repeated(panel, frames, values) {
  const selected = values[panel.repeat];
  const filter = panel.transformations.find((t) => t.id === "filterByValue").options.filters[0];
  const counts = selected.values.map(() => 0);
  let other = 0;
  for (const v of column(frames, filter.fieldName)) {
    const i = selected.values.indexOf(v);
    if (i < 0) other++;
    else counts[i]++;
  }
  if (other && selected.every) throw new Error(`${other} rows in no panel of ${panel.repeat}`);
  const rest = selected.every ? "" : `, ${other} of values not selected`;
  return `, ${selected.values.length} panels by ${panel.repeat} (${counts.join(" + ") || 0} rows${rest})`;
}

/** What a regex or the JSON of a transformation reads specially. */
const SPECIAL = /[\\^$.*+?()[\]{}|"]/;

/** A panel's filters on a variable's selection: { variable, column, value }. */
export function followed(panel) {
  return (panel.transformations ?? [])
    .filter((t) => t.id === "filterByValue")
    .flatMap((t) => t.options.filters)
    .filter((f) => f.config.id === "regex")
    .flatMap((f) => {
      const m = f.config.options.value.match(/\$\{(\w+):pipe\}/);
      return m ? [{ variable: m[1], column: f.fieldName, value: f.config.options.value }] : [];
    });
}

/**
 * The rows a panel following a variable keeps, by Grafana's regex value
 * matcher (`new RegExp(value).test(cell)`, the value interpolated in the
 * transformation's JSON by `interpolate`): fails unless they are exactly
 * the rows of the selected values.
 */
export function following(panel, frames, values, interpolate) {
  let out = "";
  for (const f of followed(panel)) {
    const selected = values[f.variable].values;
    const odd = selected.filter((v) => SPECIAL.test(v)).length;
    if (odd) throw new Error(`${odd} values of ${f.variable} a regex or JSON reads specially`);
    const matcher = new RegExp(interpolate(f.value, values));
    const cells = column(frames, f.column);
    const kept = cells.filter((v) => v != null && matcher.test(v)).length;
    const expected = cells.filter((v) => selected.includes(v)).length;
    if (kept !== expected) {
      throw new Error(`keeps ${kept} rows of ${f.variable}, its selection has ${expected}`);
    }
    out += `, ${kept} of ${cells.length} rows kept by ${selected.length} ${f.variable}`;
  }
  return out;
}
