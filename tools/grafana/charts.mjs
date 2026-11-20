// The app's charts as panels (decision 0031): Grafana runs the same SQL files
// the API runs (crates/quality/charts/<id>.sql), never a copy. Pure
// functions; dashboards.mjs reads the files.
//
// A chart panel of the definition is { chart: "<id>", w, h } and, to draw
// the figure the app draws (figures.mjs):
//   as           a panel type, when the kind's does not read well
//   x            the category or x column, the first by default
//   orientation  a bar chart's "horizontal" bars (the default) or "vertical"
//   stacking     a bar chart's "normal" or "percent" stacks
//   repeat       a dashboard variable: a panel per value, its rows those
//                whose column `filter` has it (as the app draws a chart
//                per dimension or variable)
//   follow       a dashboard variable: the rows whose column `filter` has
//                one of its selected values (the tables of repeated charts)
//   rows         [{ column, is: "notNull" | "above", value }]: rows kept
//   sort, limit  { column, desc } and the number of rows drawn
//   label        { columns, value }: a bar per row named `a · b`
//   matrix       { rows, columns, value, head, scheme }: a table of cells
//   split        a column whose values each name a series (`legend`)
//   coalesce     { columns, label, … }: the last column with a value
//   position     { value, low, high, as, steps }: where a value lies
//                between its limits, its colour by the steps
//   series       an x-y chart's { x, points, color, lines }
//   note         a key of the web's `charts` texts, shown as the panel's
//                description; `explain` a key of the catalogues
//   columns      { column: { label, values, unit, decimals, w, min, max,
//                softMin, bar, hide, show, text, color, thresholds,
//                thresholdsStyle, legend } } (panels.mjs).
// Every column the query returns is
// named from the web's texts (packages/feature-quality/src/locales, under
// `charts.`): its `label`, else `chartColumns.labels` of the definition,
// else its own name; failing those, `columns.<column>` of the definition's
// catalogue. `chartColumns.values` names, by column, the web's texts its
// values are shown with.

/** How each kind of the catalogue is drawn by default. */
const KIND_PANEL = { line: "trend", bar: "barchart", scatter: "xychart", table: "table" };
const PARAMETERS = new Set(["dataset", "cut", "previous_cut"]);

/**
 * The catalogue of crates/quality/src/charts.rs, read strictly: every
 * `Chart { id, kind, needs, sql: include_str!("../charts/<id>.sql") }` of
 * CATALOG, in order.
 */
export function parseCatalog(rust) {
  const block = rust.match(/pub const CATALOG: &\[Chart\] = &\[\n([\s\S]*?)\n\];/);
  if (!block) throw new Error("charts.rs: CATALOG not found");
  const entry =
    /^ {4}Chart \{\n {8}id: "(\w+)",\n {8}kind: "(\w+)",\n {8}needs: &\[([^\]]*)\],\n {8}sql: include_str!\("\.\.\/charts\/(\w+)\.sql"\),\n {4}\},?$/gm;
  const charts = [...block[1].matchAll(entry)].map(([, id, kind, needs, file]) => {
    if (file !== id) throw new Error(`charts.rs: ${id} reads ${file}.sql`);
    if (!KIND_PANEL[kind]) throw new Error(`charts.rs: ${id} has kind ${kind}`);
    const roles = needs.split(",").map((n) => n.trim());
    return { id, kind, needs: roles.filter(Boolean).map((n) => JSON.parse(n)) };
  });
  const declared = block[1].match(/^ {4}Chart \{$/gm)?.length ?? 0;
  if (charts.length !== declared) throw new Error("charts.rs: a CATALOG entry could not be read");
  return charts;
}

/**
 * The SQL with each named parameter as a Grafana variable, by the scan of
 * `charts::bind`: `:name` not preceded by another `:`; an unknown name is an
 * error. `bind` passes each value as a text parameter, absent as NULL; here
 * the variable's quoted value is that text, and an empty one (no previous
 * cut, or no value at all) is NULL. Parenthesized, so `::date` casts it.
 */
export function substitute(sql) {
  let out = "";
  let i = 0;
  while (i < sql.length) {
    const starts = sql[i] === ":" && /[A-Za-z_]/.test(sql[i + 1] ?? "") && sql[i - 1] !== ":";
    if (!starts) {
      out += sql[i++];
      continue;
    }
    let end = i + 1;
    while (end < sql.length && /\w/.test(sql[end])) end++;
    const name = sql.slice(i + 1, end);
    if (!PARAMETERS.has(name)) throw new Error(`unknown parameter :${name}`);
    out += `(NULLIF(\${${name}:sqlstring}, '')::text)`;
    i = end;
  }
  return out;
}

/** The names of the columns the query's outermost SELECT returns, in order. */
export function outputColumns(sql) {
  const text = sql.replace(/--[^\n]*/g, "");
  let depth = 0;
  let quoted = false;
  let select = -1;
  let from = -1;
  const commas = [];
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (c === "'") quoted = !quoted;
    if (quoted) continue;
    if (c === "(") depth++;
    if (c === ")") depth--;
    if (depth !== 0) continue;
    const word = (w) =>
      new RegExp(`^${w}\\b`, "i").test(text.slice(i)) && !/\w/.test(text[i - 1] ?? " ");
    if (word("SELECT")) [select, from, commas.length] = [i + 6, -1, 0];
    else if (select >= 0 && from < 0 && word("FROM")) from = i;
    else if (select >= 0 && from < 0 && c === ",") commas.push(i);
  }
  if (select < 0 || from < 0) throw new Error("no outermost SELECT … FROM");
  const cuts = [select, ...commas.map((c) => c + 1)];
  return cuts.map((start, k) => {
    const item = text.slice(start, k + 1 < cuts.length ? cuts[k + 1] - 1 : from).trim();
    const name = item.match(/\bAS\s+(\w+)$/i)?.[1] ?? item.match(/^(?:\w+\.)?(\w+)$/)?.[1];
    if (!name) throw new Error(`cannot name the column ${item}`);
    return name;
  });
}

/** The web's `charts` texts of a language, flattened as `charts.<key>`. */
export function webTexts(locale) {
  const out = {};
  const walk = (prefix, node) => {
    for (const [k, v] of Object.entries(node)) {
      if (typeof v === "string") out[`${prefix}.${k}`] = v;
      else walk(`${prefix}.${k}`, v);
    }
  };
  walk("charts", locale.charts);
  return out;
}

/**
 * The value-mapping keys of the web's texts under `charts.<group>`; a single
 * text (`emptyGroup`) names the empty value.
 */
function valueKeys(group, web) {
  if (typeof web[`charts.${group}`] === "string") return { "": `charts.${group}` };
  const prefix = `charts.${group}.`;
  const keys = Object.keys(web).filter((k) => k.startsWith(prefix));
  if (!keys.length) throw new Error(`the web has no texts charts.${group}`);
  return Object.fromEntries(keys.map((k) => [k.slice(prefix.length), k]));
}

/**
 * A chart panel as a panel of the definition: its type, its SQL text, its
 * title and description keys and every column with the key it is named by.
 */
export function chartPanel(p, { catalog, sqlOf, web, chartColumns }) {
  const chart = catalog.find((c) => c.id === p.chart);
  if (!chart) throw new Error(`no chart ${p.chart} in the catalogue`);
  const sql = sqlOf(chart.id);
  const names = outputColumns(sql);
  const { labels = {}, values = {} } = chartColumns ?? {};
  const unknown = [...Object.keys(p.columns ?? {}), ...referenced(p)].filter(
    (c) => !names.includes(c),
  );
  if (unknown.length) throw new Error(`chart ${chart.id} returns no ${unknown}`);
  if (Boolean(p.repeat || p.follow) !== Boolean(p.filter) || (p.repeat && p.follow)) {
    throw new Error(`chart ${chart.id}: filter goes with one of repeat or follow`);
  }
  const columns = Object.fromEntries(
    names.map((name) => {
      const spec = { ...p.columns?.[name] };
      const key = `charts.${spec.label ?? labels[name] ?? name}`;
      spec.label = typeof web[key] === "string" ? key : `columns.${name}`;
      const group = spec.values ?? values[name];
      if (group) spec.values = valueKeys(group, web);
      return [name, spec];
    }),
  );
  const out = {
    ...p,
    type: p.as ?? KIND_PANEL[chart.kind],
    title: `charts.${chart.id}`,
    sqlText: substitute(sql),
    columns,
    x: p.x ?? names[0],
    description: [p.note && `charts.${p.note}`, p.explain, chart.needs.length && "needs"].filter(
      Boolean,
    ),
    needs: chart.needs,
  };
  for (const key of ["chart", "as", "note", "explain"]) delete out[key];
  return out;
}

/** The query's columns a chart panel's keys name. */
function referenced(p) {
  const { label, matrix, coalesce, position, series } = p;
  return [
    p.x,
    p.split,
    p.filter,
    p.sort?.column,
    ...(p.rows ?? []).map((r) => r.column),
    ...(label ? [...label.columns, label.value] : []),
    ...(matrix ? [matrix.rows, matrix.columns, matrix.value] : []),
    ...(coalesce?.columns ?? []),
    ...(position ? [position.value, position.low, position.high] : []),
    ...(series ? [series.x, series.points, ...(series.lines ?? [])] : []),
  ].filter(Boolean);
}

/** What only a figure uses of a column; a table shows the column as it is. */
const FIGURE_ONLY = new Set([
  "hide",
  "show",
  "color",
  "thresholds",
  "thresholdsStyle",
  "softMin",
  "legend",
]);

/**
 * A collapsed row of that title with a table of every row of each chart
 * the dashboard draws as a figure, as the app puts a table under each; a
 * repeated chart's table keeps the rows of the values selected in its
 * variable, all of them at All.
 */
export function chartTables(panels, title, ctx) {
  const tables = panels
    .filter((p) => p.chart && chartPanel(p, ctx).type !== "table")
    .map((p) => {
      const columns = Object.entries(p.columns ?? {}).map(([c, spec]) => [
        c,
        Object.fromEntries(Object.entries(spec).filter(([k]) => !FIGURE_ONLY.has(k))),
      ]);
      const table = {
        chart: p.chart,
        w: 12,
        h: 8,
        as: "table",
        columns: Object.fromEntries(columns),
        ...(p.repeat ? { follow: p.repeat, filter: p.filter } : {}),
      };
      return chartPanel(table, ctx);
    });
  return { type: "row", title, w: 24, h: 1, panels: tables };
}
