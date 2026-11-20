// The app's figures as Grafana panels (decision 0031): what a chart panel
// changes in its rows before it is drawn, with stock transformations only,
// and the look of a bar chart and an x-y chart. charts.mjs describes the
// keys of a chart panel; panels.mjs calls these.

/** The column a `label` panel's value goes to: Grafana names a series by its frame then. */
const VALUE = "Value";

/** A filterByValue transformation keeping (or dropping) the rows of one matcher. */
const filterRows = (fieldName, config, type = "include") => ({
  id: "filterByValue",
  options: { filters: [{ fieldName, config }], match: "any", type },
});

/** A row filter of the definition: { column, is: "notNull" | "above", value }. */
function rowFilter({ column, is, value }) {
  if (is === "notNull") return filterRows(column, { id: "isNull", options: {} }, "exclude");
  if (is === "above") return filterRows(column, { id: "greater", options: { value } });
  throw new Error(`unknown row filter ${is}`);
}

/** A binary calculated field `left op right`, by the columns' names. */
const binary = (left, operator, right, alias) => ({
  id: "calculateField",
  options: {
    alias,
    binary: {
      left: { matcher: { id: "byName", options: left } },
      operator,
      right: { matcher: { id: "byName", options: right } },
    },
    mode: "binary",
  },
});

/** The calculated fields a p′ chart's points are coloured by, then dropped. */
export const POSITION_PARTS = ["p_from_lcl", "lcl_to_ucl"];

/**
 * What a panel computes from its columns: `coalesce` ({ columns, label })
 * is the last of those columns that has a value, named by the catalogue's
 * `label`;
 * `position` ({ value, low, high }) is where the value lies between its
 * limits (below 0, within 0 to 1, above 1), for colouring the points.
 */
function calculations(p, t) {
  const out = [];
  if (p.coalesce) {
    const { columns, label } = p.coalesce;
    out.push({
      id: "calculateField",
      options: {
        alias: t(label),
        mode: "reduceRow",
        reduce: { include: columns, reducer: "lastNotNull" },
      },
    });
  }
  if (p.position) {
    const { value, low, high, as } = p.position;
    const [fromLow, width] = POSITION_PARTS;
    out.push(binary(value, "-", low, fromLow), binary(high, "-", low, width));
    out.push(binary(fromLow, "/", width, as));
  }
  return out;
}

/**
 * The rows a panel keeps and their order, before anything is renamed:
 * those of its repeat's value (`filter`), or of any value selected in the
 * variable it follows (`follow`: a regex of the values joined by `|`,
 * anchored, as All gives every value of a variable with no All value of
 * its own), those its `rows` filters keep, sorted by `sort`
 * ({ column, desc }) and the first `limit`.
 */
export function selection(p, t) {
  const out = [];
  if (p.repeat)
    out.push(filterRows(p.filter, { id: "equal", options: { value: `\${${p.repeat}}` } }));
  if (p.follow)
    out.push(filterRows(p.filter, { id: "regex", options: { value: `^(\${${p.follow}:pipe})$` } }));
  for (const f of p.rows ?? []) out.push(rowFilter(f));
  if (p.sort)
    out.push({
      id: "sortBy",
      options: { sort: [{ field: p.sort.column, desc: Boolean(p.sort.desc) }] },
    });
  if (p.limit) out.push({ id: "limit", options: { limitField: p.limit } });
  return [...out, ...calculations(p, t)];
}

/**
 * A bar per row named by several columns (`label`: { columns, value }):
 * one frame per row, named `a · b`, reduced back to one row each, so the
 * bar chart's category is that name.
 */
export function labelled(p) {
  const { columns, value } = p.label;
  const excludeByName = Object.fromEntries(
    Object.keys(p.columns)
      .filter((c) => c !== value && !columns.includes(c))
      .map((c) => [c, true]),
  );
  return [
    {
      id: "organize",
      options: { excludeByName, indexByName: {}, renameByName: { [value]: VALUE } },
    },
    {
      id: "partitionByValues",
      options: {
        fields: columns,
        keepFields: false,
        naming: { asLabels: false, separator2: " · " },
      },
    },
    {
      id: "reduce",
      options: { includeTimeField: false, mode: "seriesToRows", reducers: ["lastNotNull"] },
    },
  ];
}

/**
 * Rows by columns (`matrix`: { rows, columns, value, head }): a cell per
 * pair, the columns in the order of their values' texts and named by
 * them, the first column named by `head`.
 */
export function matrix(p, t) {
  const { rows, columns, value, head } = p.matrix;
  const values = p.columns[columns].values ?? {};
  const renameByName = { [`${rows}\\${columns}`]: t(`charts.${head}`) };
  const indexByName = { [`${rows}\\${columns}`]: 0 };
  Object.entries(values).forEach(([v, key], i) => {
    renameByName[v] = t(key);
    indexByName[v] = i + 1;
  });
  return [
    {
      id: "groupingToMatrix",
      options: { columnField: columns, emptyValue: "zero", rowField: rows, valueField: value },
    },
    { id: "organize", options: { excludeByName: {}, indexByName, renameByName } },
  ];
}

/** Value mappings for every `a · b` a `label` panel's category can take. */
export function labelValues(p) {
  const groups = p.label.columns.map((c) => Object.entries(p.columns[c].values ?? {}));
  if (groups.some((g) => !g.length)) return undefined;
  let out = [["", []]];
  for (const g of groups) {
    out = out.flatMap(([v, keys]) => g.map(([w, key]) => [v ? `${v} · ${w}` : w, [...keys, key]]));
  }
  return out;
}

/**
 * The look of a bar chart: bars across (the default) or columns, stacked or
 * not; from zero unless the panel's `min` is null (values below it).
 */
export function barchart(p, legend, fieldOverrides, xField) {
  const defaults = { ...(p.min === null ? {} : { min: 0 }), unit: p.unit ?? "short" };
  if (p.decimals !== undefined) defaults.decimals = p.decimals;
  return {
    fieldConfig: { defaults, overrides: fieldOverrides },
    options: {
      legend: { ...legend, showLegend: Boolean(p.legendShown) },
      orientation: p.orientation ?? "horizontal",
      showValue: "auto",
      ...(p.stacking ? { stacking: p.stacking } : {}),
      tooltip: { mode: p.stacking ? "multi" : "single", sort: "none" },
      ...(xField && !p.label ? { xField } : {}),
    },
  };
}

/**
 * An x-y chart's series (`series`: { x, points, color, lines }): the
 * points coloured by the `color` column, then a line for each of `lines`,
 * by the names the panel shows its columns with.
 */
export function xySeries(p, name) {
  const { x, points, color, lines = [] } = p.series;
  const one = (y, extra = {}) => ({
    frame: { matcher: { id: "byIndex", options: 0 } },
    x: { matcher: { id: "byName", options: name(x) } },
    y: { matcher: { id: "byName", options: name(y) } },
    ...extra,
  });
  return [
    one(points, color ? { color: { matcher: { id: "byName", options: color } } } : {}),
    ...lines.map((l) => one(l)),
  ];
}
