// The field config of a panel (lib.mjs describes the definition): each
// column's unit, decimals, value texts, colour and thresholds by the name it
// is shown with, a table's link to another dashboard of the same language,
// and the fields a chart panel computes or reshapes (figures.mjs).

import { labelValues } from "./figures.mjs";

/** A column spec: a width (or null), or { w, unit, decimals, … } (charts.mjs). */
const columnSpec = (spec) => (spec !== null && typeof spec === "object" ? spec : { w: spec });

/** The catalogue key a column is named by. */
export const labelKey = (alias, spec) => columnSpec(spec).label ?? `columns.${alias}`;

/** A value mapping from [value, text] pairs, and "" (an empty value) to its text. */
function valueMapping(pairs, empty) {
  const out = [];
  const options = pairs.map(([v, text], index) => [v, { index, text }]);
  if (options.length) out.push({ type: "value", options: Object.fromEntries(options) });
  for (const match of empty ? ["empty", "null"] : []) {
    out.push({ type: "special", options: { match, result: { text: empty } } });
  }
  return out;
}

/** A value mapping from the keys of a column's values; "" is an empty value. */
export function mappings(values, t) {
  const { "": empty, ...rest } = values;
  const pairs = Object.entries(rest).map(([v, key]) => [v, t(key)]);
  return valueMapping(pairs, empty && t(empty));
}

/** Hidden from the figure (`hide: true`), or from its tooltip too (`"all"`). */
const HIDDEN = {
  true: { legend: true, tooltip: false, viz: true },
  all: { legend: true, tooltip: true, viz: true },
};

/** Thresholds from [value, colour] steps, the first's value null (the base). */
const thresholds = (steps) => ({
  mode: "absolute",
  steps: steps.map(([value, color]) => ({ color, value })),
});

/** A column's own field properties beyond its width (charts.mjs names them). */
function properties(spec, t) {
  const { unit, decimals, min, max, bar, hide, show, values, color, softMin } = spec;
  const out = [];
  if (unit) out.push({ id: "unit", value: unit });
  if (decimals !== undefined) out.push({ id: "decimals", value: decimals });
  if (min !== undefined) out.push({ id: "min", value: min });
  if (max !== undefined) out.push({ id: "max", value: max });
  if (softMin !== undefined) out.push({ id: "custom.axisSoftMin", value: softMin });
  if (bar) out.push({ id: "custom.cellOptions", value: { type: "gauge", mode: "basic" } });
  if (hide) out.push({ id: "custom.hideFrom", value: HIDDEN[hide] });
  if (show) out.push({ id: "custom.show", value: show });
  if (values) out.push({ id: "mappings", value: mappings(values, t) });
  if (color) out.push({ id: "color", value: { mode: "fixed", fixedColor: color } });
  if (spec.thresholds) {
    out.push({ id: "color", value: { mode: "thresholds" } });
    out.push({ id: "thresholds", value: thresholds(spec.thresholds) });
  }
  if (spec.thresholdsStyle) {
    out.push({ id: "custom.thresholdsStyle", value: { mode: spec.thresholdsStyle } });
  }
  return out;
}

/**
 * A table's link to `d/qd-<dashboard>-<lang>`. Grafana resolves a link that
 * starts with `/` under its sub-path, and `${__data.fields.<alias>}` finds a
 * field by the name the query gave it, before the `organize` rename.
 */
export function linkUrl(link, lang) {
  const field = (alias) => `\${__data.fields.${alias}}`;
  const params = Object.entries(link.vars).map(([v, alias]) => `var-${v}=${field(alias)}`);
  if (link.from) params.push(`from=${field(link.from)}`, `to=${field(link.to)}`);
  return `/d/qd-${link.dashboard}-${lang}?${params.join("&")}`;
}

/**
 * A `label` panel's two fields after the reduce: the category (named
 * `a · b`, each part shown by its text) and the value.
 */
function labelOverrides(p, t) {
  const { value } = p.label;
  const pairs = (labelValues(p) ?? []).map(([v, keys]) => [v, keys.map(t).join(" · ")]);
  const out = [];
  if (pairs.length) {
    out.push({
      matcher: { id: "byType", options: "string" },
      properties: [{ id: "mappings", value: valueMapping(pairs) }],
    });
  }
  const spec = columnSpec(p.columns[value]);
  const props = [{ id: "displayName", value: t(labelKey(value, spec)) }, ...properties(spec, t)];
  out.push({ matcher: { id: "byType", options: "number" }, properties: props });
  return out;
}

/** A split panel's series name: the column's, or its `legend` text with the split value. */
function splitName(p, alias, spec, t) {
  const label = t(labelKey(alias, spec));
  if (!spec.legend) return label;
  const value = `\${__field.labels.${p.split}}`;
  return t(spec.legend).replace("{series}", label).replace("{value}", value);
}

/** Overrides of the fields a panel computes (figures.mjs). */
function computed(p, t) {
  const out = [];
  if (p.coalesce) {
    const { label, ...spec } = p.coalesce;
    delete spec.columns;
    out.push({
      matcher: { id: "byName", options: t(label) },
      properties: properties(spec, t),
    });
  }
  if (p.position) {
    const spec = { hide: "all", thresholds: p.position.steps };
    out.push({
      matcher: { id: "byName", options: p.position.as },
      properties: properties(spec, t),
    });
  }
  return out;
}

/** Field overrides by the renamed column: width, unit, decimals, link, hidden. */
export function overrides(p, t, lang) {
  if (p.label) return labelOverrides(p, t);
  const out = [];
  if (p.matrix) {
    const { rows, head } = p.matrix;
    const values = columnSpec(p.columns[rows]).values;
    // The rows' names are not coloured as the cells are.
    const props = [{ id: "custom.cellOptions", value: { type: "auto" } }];
    if (values) props.push({ id: "mappings", value: mappings(values, t) });
    out.push({ matcher: { id: "byName", options: t(`charts.${head}`) }, properties: props });
    return out;
  }
  for (const [alias, raw] of Object.entries(p.columns ?? {})) {
    const spec = columnSpec(raw);
    const props = properties(spec, t);
    if (spec.w && p.type === "table") props.unshift({ id: "custom.width", value: spec.w });
    if (p.link?.column === alias) {
      const title = t(`${p.title}.link`);
      props.push({ id: "links", value: [{ title, url: linkUrl(p.link, lang) }] });
    }
    // Split rows keep their own names: each series is named by its column.
    if (p.split) props.unshift({ id: "displayName", value: splitName(p, alias, spec, t) });
    if (props.length) {
      const name = p.split ? alias : t(labelKey(alias, raw));
      out.push({ matcher: { id: "byName", options: name }, properties: props });
    }
  }
  // The link's time range: in the rows, never shown.
  for (const alias of [p.link?.from, p.link?.to].filter(Boolean)) {
    out.push({
      matcher: { id: "byName", options: alias },
      properties: [{ id: "custom.hidden", value: true }],
    });
  }
  return [...out, ...computed(p, t)];
}
