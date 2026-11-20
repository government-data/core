// Builds Grafana dashboards from the single definition in
// infra/observability/grafana/definition (decision 0026, point 8): pure
// functions, no I/O; dashboards.mjs reads the files and writes the result.
//
// The definition (dashboards.json):
//   languages     the catalogues to build, one folder per language
//   datasources   name → { type, uid }, as provisioned
//   variables     name → { kind: "query", datasource, query, multi, all, hide }
//                 | { kind: "textbox", sample }. A query variable offers All
//                 unless `all: false`; All's value is the literal '__all'
//                 inserted as is: a query tests
//                 `'__all' IN (${v:sqlstring}) OR col IN (${v:sqlstring})`.
//                 Its query may use the variables listed before it;
//                 `repeat: true` for one panels repeat by (variables.mjs);
//                 on Mimir (`datasource: "mimir"`) the query is PromQL's
//                 `label_values(...)`.
//   dashboards    [{ slug, folder, tags, time, variables, panels }]; a
//                 variable is a library name or { name, multi, all, hide }
//                 overriding those for this dashboard (variables.mjs)
//   panel         { type, title, w, h, and one of sql: "<file>" (in
//                 queries/<slug>/), promql: "<file>" (the same, on Mimir),
//                 logql: "<expr>", or none for a text or alert list
//                 panel; type is timeseries (style "bars" stacked, the
//                 default, or "lines"), barchart, table, stat, text, logs
//                 or alertlist (Grafana's alerts whose labels match
//                 `labels`, a matcher that may use the variables);
//                 unit and decimals for the panel; legendShown: a bar
//                 chart's legend; legend: a LogQL legend
//                 format (`{{service}}`); columns: { alias: width |
//                 null | { w, unit, decimals } } renamed per language; link:
//                 { dashboard, column, vars: { variable: alias }, from, to }
//                 makes a table's column open another dashboard of the same
//                 language with those variables and, from the fields `from`
//                 and `to` (epoch ms, hidden), that time range (panels.mjs) }
//   chart panel   { chart: "<id>", w, h, … }: one of the app's charts,
//                 expanded into a panel before any of this (charts.mjs);
//                 `repeat: "<variable>"` draws it once per value of that
//                 dashboard variable, its rows filtered by `filter`
//   tables        a dashboard's `tables: "<key>"` ends it with a collapsed
//                 row of that title holding a table of every row of each
//                 chart drawn as a figure, a repeated one's rows those of
//                 its variable's selected values (charts.mjs)
// Every string a person reads is a key of locales/<language>.json:
// `folder` (or the dashboard's own folder key), `<slug>.title`, the panel
// titles, `<title>.text` for a text panel, `<title>.link` for a link,
// `variables.<name>`, `columns.<alias>` (or a column's `label`), `needs`
// and `description`; a chart panel's keys under `charts.` are the web's.

import { labelKey, panel } from "./panels.mjs";
import { checkVariable, variable, variableName } from "./variables.mjs";

export { ALL } from "./variables.mjs";

/** Grafana 13.2.5's latest dashboard schema (its migrator's last step). */
export const SCHEMA_VERSION = 42;
const GRID = 24;
const PANEL_TYPES = new Set([
  "timeseries",
  "barchart",
  "table",
  "stat",
  "text",
  "logs",
  "trend",
  "xychart",
  "alertlist",
  "row",
]);
/** The panel types that read no query. */
const NO_QUERY = new Set(["text", "alertlist"]);

/** Fails unless every catalogue has the same keys. */
export function checkCatalogues(catalogues) {
  const [first, ...rest] = Object.keys(catalogues);
  const keys = new Set(Object.keys(catalogues[first]));
  for (const lang of rest) {
    const other = new Set(Object.keys(catalogues[lang]));
    const missing = [...keys].filter((k) => !other.has(k));
    const extra = [...other].filter((k) => !keys.has(k));
    if (missing.length || extra.length) {
      throw new Error(
        `catalogues ${first} and ${lang} differ: only in ${first}: ${missing.join(", ") || "none"}; ` +
          `only in ${lang}: ${extra.join(", ") || "none"}`,
      );
    }
  }
}

/** Fails unless a table's link names a dashboard, its variables and columns. */
function checkLink(p, def, where) {
  const { dashboard: slug, column, vars, from, to } = p.link;
  const target = def.dashboards.find((d) => d.slug === slug);
  if (!target) throw new Error(`${where}: link to unknown dashboard ${slug}`);
  if (p.type !== "table") throw new Error(`${where}: only a table links`);
  if (!(column in (p.columns ?? {}))) throw new Error(`${where}: link column ${column} unknown`);
  const names = new Set((target.variables ?? []).map(variableName));
  const unknown = Object.keys(vars ?? {}).filter((v) => !names.has(v));
  if (unknown.length) throw new Error(`${where}: ${slug} has no variable ${unknown}`);
  if (Boolean(from) !== Boolean(to)) throw new Error(`${where}: link needs both from and to`);
}

/** The keys and SQL files a definition refers to, failing on a malformed panel. */
export function references(def) {
  const keys = new Set(["description"]);
  const files = new Set();
  for (const name of Object.keys(def.variables)) keys.add(`variables.${name}`);
  for (const d of def.dashboards) {
    keys.add(d.folder ?? "folder");
    keys.add(`${d.slug}.title`);
    for (const v of d.variables ?? []) checkVariable(v, def.variables, d.slug);
    const names = new Set((d.variables ?? []).map(variableName));
    // Rows come after every other panel; each holds its own panels.
    const first = d.panels.findIndex((p) => p.type === "row");
    if (first >= 0 && d.panels.slice(first).some((p) => p.type !== "row")) {
      throw new Error(`${d.slug}: rows only after every other panel`);
    }
    for (const p of d.panels.flatMap((x) => [x, ...(x.panels ?? [])])) {
      const where = `${d.slug}: panel ${p.title}`;
      if (!PANEL_TYPES.has(p.type)) throw new Error(`${where}: unknown type ${p.type}`);
      if (p.chart) throw new Error(`${where}: a chart panel must be expanded first`);
      for (const v of [p.repeat, p.follow].filter(Boolean)) {
        if (!names.has(v)) throw new Error(`${where}: no variable ${v}`);
      }
      if (p.type === "row") {
        keys.add(p.title);
        continue;
      }
      if (p.link) {
        checkLink(p, def, where);
        keys.add(`${p.title}.link`);
      }
      const sources = [p.sql, p.promql, p.logql, p.sqlText].filter(Boolean).length;
      if (sources !== (NO_QUERY.has(p.type) ? 0 : 1)) {
        throw new Error(`${where}: needs exactly one of sql, promql or logql (none for text)`);
      }
      if ((p.type === "alertlist") !== Boolean(p.labels)) {
        throw new Error(`${where}: an alert list, and only one, has labels`);
      }
      if (!(p.w > 0 && p.w <= GRID && p.h > 0)) throw new Error(`${where}: bad size`);
      keys.add(p.title);
      if (p.type === "text") keys.add(`${p.title}.text`);
      if (p.sql) files.add(`${d.slug}/${p.sql}`);
      if (p.promql) files.add(`${d.slug}/${p.promql}`);
      for (const [alias, spec] of Object.entries(p.columns ?? {})) {
        keys.add(labelKey(alias, spec));
        for (const key of Object.values(spec?.values ?? {})) keys.add(key);
        if (spec?.legend) keys.add(spec.legend);
      }
      if (p.coalesce) keys.add(p.coalesce.label);
      if (p.matrix) keys.add(`charts.${p.matrix.head}`);
      for (const key of p.description ?? []) keys.add(key);
    }
  }
  return { keys, files };
}

/** Fails if a key the definition uses is missing from a catalogue. */
export function checkKeys(keys, catalogues) {
  for (const [lang, catalogue] of Object.entries(catalogues)) {
    const missing = [...keys].filter((k) => typeof catalogue[k] !== "string");
    if (missing.length) throw new Error(`catalogue ${lang} lacks: ${missing.join(", ")}`);
  }
}

/**
 * Grid positions from `top`: panels flow left to right and wrap at 24
 * columns; a row starts a line of its own, its panels below it.
 */
function layout(panels, top = 0) {
  let x = 0;
  let y = top;
  let row = 0;
  return panels.map((p) => {
    if (x + p.w > GRID || p.type === "row") {
      x = 0;
      y += row;
      row = 0;
    }
    const pos = { h: p.h, w: p.w, x, y };
    x += p.w;
    row = Math.max(row, p.h);
    return pos;
  });
}

/** The panels with their ids and positions, a row's own after it. */
function placed(panels, ctx) {
  let id = 0;
  const positions = layout(panels);
  // Rows stack below the other panels, each as tall as its own.
  const flat = positions.filter((_, i) => panels[i].type !== "row");
  let top = Math.max(0, ...flat.map((q) => q.y + q.h));
  return panels
    .map((p, i) => {
      if (p.type !== "row") return [panel(p, ++id, positions[i], ctx)];
      const own = layout(p.panels, top);
      const out = {
        id: ++id,
        type: "row",
        title: ctx.t(p.title),
        gridPos: { ...positions[i], y: top },
      };
      const inner = p.panels.map((q, j) => panel(q, ++id, { ...own[j], y: own[j].y + 1 }, ctx));
      top += 1 + Math.max(0, ...own.map((q) => q.y + q.h - top));
      // An open row's panels follow it; a collapsed one holds them.
      if (p.open) return [{ ...out, collapsed: false, panels: [] }, ...inner];
      return [{ ...out, collapsed: true, panels: inner }];
    })
    .flat();
}

/** One dashboard in one language. */
export function dashboard(d, lang, { def, t, sqlOf }) {
  const uid = `qd-${d.slug}-${lang}`;
  if (uid.length > 40) throw new Error(`uid ${uid} is longer than 40 characters`);
  const ctx = { t, sqlOf, datasources: def.datasources, slug: d.slug, lang };
  return {
    annotations: {
      list: [
        {
          builtIn: 1,
          datasource: { type: "grafana", uid: "-- Grafana --" },
          enable: true,
          hide: true,
          iconColor: "rgba(0, 211, 255, 1)",
          name: "Annotations & Alerts",
          type: "dashboard",
        },
      ],
    },
    description: t("description"),
    editable: false,
    fiscalYearStartMonth: 0,
    graphTooltip: 1,
    id: null,
    links: [],
    panels: placed(d.panels, ctx),
    refresh: "",
    schemaVersion: SCHEMA_VERSION,
    tags: d.tags ?? [],
    templating: {
      list: (d.variables ?? []).map((v) => variable(v, def.variables, t, def.datasources)),
    },
    time: { from: d.time ?? "now-7d", to: "now" },
    timepicker: d.timepicker === false ? { hidden: true } : {},
    timezone: "browser",
    title: t(`${d.slug}.title`),
    uid,
    weekStart: "",
  };
}
