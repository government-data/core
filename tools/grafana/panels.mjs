// The panels of a dashboard (lib.mjs describes the definition): each type's
// look, its query and the per-language renames of its columns; their field
// config is in fields.mjs, and a chart panel's figure (its transformations,
// a bar chart's and an x-y chart's look) in figures.mjs.

import { labelKey, overrides } from "./fields.mjs";
import { barchart, labelled, matrix, POSITION_PARTS, selection, xySeries } from "./figures.mjs";

export { labelKey, linkUrl } from "./fields.mjs";

/** Grafana's version: an x-y chart without it is read as the old one. */
const GRAFANA = "13.2.5";

function timeseries(p) {
  const lines = p.style === "lines";
  return {
    custom: lines
      ? { drawStyle: "line", fillOpacity: 0, lineWidth: 1, showPoints: "always", spanNulls: true }
      : {
          drawStyle: "bars",
          fillOpacity: 80,
          lineWidth: 1,
          stacking: { group: "A", mode: "normal" },
        },
    // A value that can be negative (a clock's offset) has no floor.
    ...(p.signed ? {} : { min: 0 }),
    unit: p.unit ?? "short",
  };
}

/** The look of a trend (lines over a numeric x) or an x-y chart (points). */
function plot(p, legend, fieldOverrides, xField, t) {
  const trend = p.type === "trend";
  const custom = trend
    ? { drawStyle: "line", fillOpacity: 0, lineWidth: 2, pointSize: 6, showPoints: "always" }
    : { pointSize: { fixed: 6 }, show: "points" };
  const range = {
    ...(p.min !== undefined ? { min: p.min } : {}),
    ...(p.max ? { max: p.max } : {}),
  };
  const name = (c) => (p.columns[c] ? t(labelKey(c, p.columns[c])) : c);
  const series = p.series
    ? { mapping: "manual", series: xySeries(p, name) }
    : { mapping: "auto", series: [] };
  return {
    fieldConfig: {
      defaults: { custom, unit: p.unit ?? "short", ...range },
      overrides: fieldOverrides,
    },
    options: {
      legend,
      tooltip: { mode: trend ? "multi" : "single", sort: "none" },
      ...(trend ? { xField } : series),
    },
  };
}

/** A table's cells: filterable, or coloured by their value (a matrix). */
function table(p, fieldOverrides) {
  const custom = p.matrix
    ? { cellOptions: { type: "color-background", mode: "gradient" } }
    : { filterable: true };
  const color = p.matrix ? { color: { mode: p.matrix.scheme } } : {};
  return {
    fieldConfig: { defaults: { custom, ...color }, overrides: fieldOverrides },
    options: { cellHeight: "sm", showHeader: true },
  };
}

/** Options and field config of each panel type. */
function look(p, t, lang) {
  const columns = Object.keys(p.columns ?? {});
  const legend = { calcs: [], displayMode: "list", placement: "bottom", showLegend: true };
  const fieldOverrides = overrides(p, t, lang);
  const x = p.x ?? columns[0];
  const xField = x && t(labelKey(x, p.columns[x]));
  switch (p.type) {
    case "trend":
    case "xychart":
      return plot(p, legend, fieldOverrides, xField, t);
    case "timeseries":
      return {
        fieldConfig: { defaults: timeseries(p), overrides: fieldOverrides },
        options: { legend, tooltip: { mode: "multi", sort: "desc" } },
      };
    case "barchart":
      return barchart(p, legend, fieldOverrides, xField);
    case "table":
      return table(p, fieldOverrides);
    case "stat":
      return {
        fieldConfig: {
          defaults: {
            color: { mode: "fixed", fixedColor: "text" },
            unit: p.unit ?? "short",
            ...(p.decimals !== undefined ? { decimals: p.decimals } : {}),
          },
          overrides: fieldOverrides,
        },
        options: {
          colorMode: "none",
          graphMode: "none",
          justifyMode: "auto",
          orientation: "auto",
          reduceOptions: { calcs: ["lastNotNull"], fields: "", values: false },
          textMode: "value",
        },
      };
    case "text":
      return { options: { content: t(`${p.title}.text`), mode: "markdown" } };
    case "alertlist":
      return { options: alertlist(p) };
    default:
      return { options: { showTime: true, sortOrder: "Descending", wrapLogMessage: true } };
  }
}

/** Grafana's alerts whose labels match `p.labels`, active or not normal. */
function alertlist(p) {
  return {
    alertInstanceLabelFilter: p.labels,
    alertName: "",
    dashboardAlerts: false,
    groupBy: [],
    groupMode: "default",
    maxItems: 20,
    showInactiveAlerts: false,
    sortOrder: 1,
    stateFilter: { error: true, firing: true, noData: true, normal: false, pending: true },
    viewMode: "list",
  };
}

function target(p, sql, datasources) {
  if (p.promql) {
    return {
      datasource: datasources.mimir,
      editorMode: "code",
      expr: sql,
      ...(p.legend ? { legendFormat: p.legend } : {}),
      range: true,
      refId: "A",
    };
  }
  if (p.logql) {
    return {
      datasource: datasources.loki,
      editorMode: "code",
      expr: p.logql,
      ...(p.legend ? { legendFormat: p.legend } : {}),
      queryType: "range",
      refId: "A",
    };
  }
  return {
    datasource: datasources.postgres,
    editorMode: "code",
    format: p.type === "timeseries" ? "time_series" : "table",
    rawQuery: true,
    rawSql: sql,
    refId: "A",
  };
}

/** One panel, its texts in the catalogue's language. */
export function panel(p, id, gridPos, ctx) {
  const { t, sqlOf, datasources, slug, lang } = ctx;
  const { fieldConfig, options } = look(p, t, lang);
  const out = { id, type: p.type, title: t(p.title), gridPos };
  if (p.repeat) {
    out.title = `${out.title} · \${${p.repeat}}`;
    Object.assign(out, { repeat: p.repeat, repeatDirection: "h", maxPerRow: p.perRow ?? 2 });
  }
  if (p.type === "alertlist") {
    out.datasource = { type: "datasource", uid: "grafana" };
  } else if (p.type !== "text") {
    const file = p.sql ?? p.promql;
    out.datasource = p.promql
      ? datasources.mimir
      : p.logql
        ? datasources.loki
        : datasources.postgres;
    out.targets = [target(p, p.sqlText ?? (file && sqlOf(`${slug}/${file}`)), datasources)];
  }
  if (p.type === "timeseries") out.maxDataPoints = 100;
  if (fieldConfig) out.fieldConfig = fieldConfig;
  out.options = options;
  if (p.type === "xychart") out.pluginVersion = GRAFANA;
  const steps = transformations(p, t);
  if (steps.length) out.transformations = steps;
  if (p.description?.length)
    out.description = p.description.map((k) => describe(k, p, t)).join(" ");
  return out;
}

/** A description's text; `needs` names the roles a chart needs. */
const describe = (key, p, t) =>
  key === "needs" ? t(key).replace("{roles}", p.needs.map((r) => `\`${r}\``).join(", ")) : t(key);

/**
 * What a panel changes in its rows: the conversions first, then the rows a
 * chart keeps and what it computes (figures.mjs), then the shape it is
 * drawn in, and last the columns renamed.
 */
function transformations(p, t) {
  const columns = Object.entries(p.columns ?? {});
  if (!columns.length) return [];
  const out = [...conversions(p), ...selection(p, t)];
  if (p.label) return [...out, ...labelled(p)];
  if (p.matrix) return [...out, ...matrix(p, t)];
  if (p.split) {
    return [...out, { id: "partitionByValues", options: { fields: [p.split], keepFields: false } }];
  }
  const renameByName = Object.fromEntries(columns.map(([a, s]) => [a, t(labelKey(a, s))]));
  const excludeByName = Object.fromEntries(
    (p.position ? POSITION_PARTS : []).map((c) => [c, true]),
  );
  return [...out, { id: "organize", options: { excludeByName, indexByName: {}, renameByName } }];
}

/**
 * The conversions of a chart panel: a column shown as text (a numeric
 * category) and a trend's x (a `YYYY-MM-DD` cut) as a number of
 * milliseconds.
 */
function conversions(p) {
  const out = Object.entries(p.columns)
    .filter(([, s]) => s?.text)
    .map(([a]) => ({ targetField: a, destinationType: "string" }));
  if (p.type === "trend") {
    out.push(
      { targetField: p.x, destinationType: "time", dateFormat: "YYYY-MM-DD" },
      { targetField: p.x, destinationType: "number" },
    );
  }
  return out.length ? [{ id: "convertFieldType", options: { conversions: out, fields: {} } }] : [];
}
