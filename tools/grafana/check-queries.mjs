// Runs every query of the generated Grafana dashboards against the running
// Grafana, as its dashboards would (decision 0026, point 8):
//   NODE_EXTRA_CA_CERTS=infra/traefik/certs/dev-ca.crt \
//     node tools/grafana/check-queries.mjs --password-file secrets/grafana_admin_password
// GRAFANA_URL (default https://localhost/grafana, through Traefik) and
// GRAFANA_ADMIN_USER (default qd-emergency-admin) from the environment; the
// password from a file, or from standard input with `--password-file -`,
// never from the command line. tools/verify/observability_check.sh runs it.
//
// For each dashboard it resolves the variables by running their queries
// through /api/ds/query, then runs each panel's query over the last 30 days
// twice: every query variable at All and text boxes empty; then each at its
// first values and text boxes at their sample. A dashboard whose first
// variable takes one value and no All (a dataset) runs once per value, the
// others at All; when panels repeat by a variable, those panels run again
// with each such variable at its first value and at its first two. It
// formats the values as Grafana 13.2.5's frontend does (seen in the queries
// its dashboards send, and in its scenes' format registry): `sqlstring`
// quotes each value doubling single quotes and joins them with commas,
// `pipe` joins them with `|`; a custom All value goes in as is, whatever the
// format, and All with no value of its own is every value. It fails
// on any error, on a column the panel renames that the query does not
// return, and when two languages' dashboards differ in their queries. It
// follows each panel's transformations over the columns its query returned
// and fails on one naming a column that is not there by then; a panel
// repeated by a variable draws a panel per selected value and, at All, must
// have each of its rows in one of them; a panel following a variable (an
// anchored regex of `${v:pipe}`) must keep exactly the rows of the selected
// values, whose values must hold nothing a regex or JSON reads specially.
// It prints row counts only, never a row.

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { ALL } from "./lib.mjs";
import { followed, following, repeated } from "./selection.mjs";
import { missingColumns } from "./steps.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const dir = path.join(root, "infra/observability/grafana/dashboards");
const base = (process.env.GRAFANA_URL || "https://localhost/grafana").replace(/\/$/, "");
const user = process.env.GRAFANA_ADMIN_USER || "qd-emergency-admin";

function password() {
  const i = process.argv.indexOf("--password-file");
  const file = i > 0 ? process.argv[i + 1] : undefined;
  if (!file) throw new Error("usage: check-queries.mjs --password-file <file | ->");
  return fs.readFileSync(file === "-" ? 0 : file, "utf8").replace(/\r?\n$/, "");
}

let auth;

async function query(datasource, q) {
  const res = await fetch(`${base}/api/ds/query`, {
    method: "POST",
    headers: { Authorization: auth, "Content-Type": "application/json" },
    body: JSON.stringify({
      from: "now-30d",
      to: "now",
      queries: [{ refId: "A", datasource, intervalMs: 3_600_000, maxDataPoints: 100, ...q }],
    }),
  });
  const body = await res.json().catch(() => ({}));
  const result = body.results?.A;
  if (!res.ok || !result || result.error) {
    throw new Error(`HTTP ${res.status}: ${result?.error ?? body.message ?? "no result"}`);
  }
  return result.frames ?? [];
}

/**
 * Values of a variable's query: the `__value` field, or the first one; on
 * Mimir, `label_values(<selector>, <label>)` asked as an instant query of
 * that label's groups.
 */
async function options(v) {
  const promql =
    v.datasource.type === "prometheus" && /^label_values\((.+),\s*(\w+)\)$/.exec(v.query);
  if (promql) {
    const [, selector, label] = promql;
    const frames = await query(v.datasource, {
      expr: `group by (${label}) (${selector})`,
      instant: true,
    });
    const labels = frames.flatMap((f) => f.schema.fields.map((x) => x.labels?.[label]));
    return [...new Set(labels.filter(Boolean))].sort();
  }
  const [frame] = await query(v.datasource, { rawSql: v.query, format: "table" });
  if (!frame) return [];
  const fields = frame.schema.fields;
  const at = Math.max(
    0,
    fields.findIndex((f) => f.name === "__value"),
  );
  return (frame.data.values[at] ?? []).map(String);
}

const sqlstring = (values) => values.map((v) => `'${v.replaceAll("'", "''")}'`).join(",");
const escapeRegex = (v) => v.replace(/[\\^$*+?.()|[\]{}/]/g, "\\$&");
const regex = (values) =>
  values.length === 1 ? escapeRegex(values[0]) : `(${values.map(escapeRegex).join("|")})`;

/** The text a variable reference becomes, in the formats the dashboards use. */
function format(value, fmt) {
  if (value.all) return ALL;
  if (fmt === "sqlstring") return sqlstring(value.values);
  if (fmt === "regex") return regex(value.values);
  if (fmt === "pipe") return value.values.join("|");
  if (fmt === undefined || fmt === "raw") return value.values.join(",");
  throw new Error(`format ${fmt} is not handled by check-queries.mjs`);
}

function interpolate(text, values) {
  return text.replace(/\$\{(\w+)(?::(\w+))?\}|\$(\w+)/g, (match, braced, fmt, bare) => {
    const name = braced ?? bare;
    if (name.startsWith("__")) return match;
    if (!(name in values)) throw new Error(`unknown variable ${name}`);
    return format(values[name], fmt);
  });
}

/**
 * The values of every variable in one case, resolved in order, each query
 * with the values chosen before it, as Grafana refreshes a variable when one
 * it uses changes: query variables at All (`all`; every value when All has
 * no value of its own) or at their sample, else their first values (`pick`
 * of a multi-value one, two by default), and text boxes empty or at
 * their sample; `pin` fixes some variables' values.
 */
async function resolve(dashboard, samples, all, pin = {}, pick = undefined) {
  const out = {};
  for (const v of dashboard.templating.list) {
    const found = () => options({ ...v, query: interpolate(v.query, out) });
    if (pin[v.name]) out[v.name] = { values: [pin[v.name]] };
    else if (v.type === "textbox") out[v.name] = { values: [all ? "" : (samples[v.name] ?? "x")] };
    else if (all && v.includeAll && v.allValue !== undefined) out[v.name] = { all: true };
    else if (all && v.includeAll) out[v.name] = { values: await found(), every: true };
    else {
      const sample = samples[v.name];
      const values = await found();
      const first = values.includes(sample) ? [sample] : values.slice(0, v.multi ? (pick ?? 2) : 1);
      out[v.name] = { values: first.length ? first : ["none"] };
    }
  }
  return out;
}

/** The names of the variables a dashboard's panels repeat by or follow. */
const selectable = (d) =>
  new Set(
    allPanels(d).flatMap((p) => [
      ...(p.repeat ? [p.repeat] : []),
      ...followed(p).map((f) => f.variable),
    ]),
  );

/**
 * The cases a dashboard's queries run in, as [name, values, only]: All and
 * empty, then first values and samples; or, when its first variable takes
 * one value and no All (a dataset), once for each of its values with the
 * others at All, so every dataset's data is read, and again with the
 * variables panels repeat by or follow at one and at two values (`only`:
 * just the panels that use them).
 */
async function cases(dashboard, samples) {
  const [first] = dashboard.templating.list;
  if (first?.type !== "query" || first.includeAll || first.multi) {
    return [
      ["all", await resolve(dashboard, samples, true)],
      ["filtered", await resolve(dashboard, samples, false)],
    ];
  }
  const values = await options(first);
  const repeats = selectable(dashboard);
  const out = [];
  for (const [i, value] of values.entries()) {
    const name = `${first.name} ${i + 1}/${values.length}`;
    const pin = { [first.name]: value };
    out.push([name, await resolve(dashboard, samples, true, pin)]);
    if (!repeats.size) continue;
    for (const [label, pick] of [
      ["one", 1],
      ["two", 2],
    ]) {
      const picked = await resolve(dashboard, samples, false, pin, pick);
      const mixed = await resolve(dashboard, samples, true, pin);
      for (const v of repeats) mixed[v] = picked[v];
      out.push([`${name} ${label} ${[...repeats].join(", ")}`, mixed, repeats]);
    }
  }
  return out;
}

async function runPanel(panel, values) {
  const t = panel.targets[0];
  const q =
    t.rawSql !== undefined
      ? { rawSql: interpolate(t.rawSql, values), format: t.format }
      : { expr: interpolate(t.expr, values), queryType: t.queryType, range: t.range };
  const frames = await query(t.datasource, q);
  const names = new Set(frames.flatMap((f) => f.schema.fields.map((x) => x.name)));
  const rows = frames.reduce((n, f) => n + (f.data.values[0]?.length ?? 0), 0);
  // An empty answer has no fields to compare.
  const missing = rows ? missingColumns(panel, names) : [];
  if (missing.length) throw new Error(`columns not there: ${missing.join(", ")}`);
  const repeat = panel.repeat ? repeated(panel, frames, values) : "";
  return `${rows} rows${repeat}${following(panel, frames, values, interpolate)}`;
}

/** A dashboard's panels, those of its rows too. */
const allPanels = (d) => d.panels.flatMap((p) => [p, ...(p.panels ?? [])]);

function load() {
  const out = [];
  for (const folder of fs.readdirSync(dir, { withFileTypes: true })) {
    if (!folder.isDirectory()) continue;
    for (const file of fs.readdirSync(path.join(dir, folder.name)).sort()) {
      if (!file.endsWith(".json")) continue;
      out.push(JSON.parse(fs.readFileSync(path.join(dir, folder.name, file), "utf8")));
    }
  }
  return out.sort((a, b) => (a.uid < b.uid ? -1 : 1));
}

/** The queries of a dashboard, which must be the same in every language. */
const queries = (d) =>
  JSON.stringify([
    d.templating.list.map((v) => [v.name, v.type, v.query, v.allValue ?? null]),
    allPanels(d).map((p) => (p.targets ?? []).map((t) => t.rawSql ?? t.expr)),
  ]);

async function main() {
  auth = `Basic ${Buffer.from(`${user}:${password()}`).toString("base64")}`;
  const def = JSON.parse(
    fs.readFileSync(
      path.join(root, "infra/observability/grafana/definition/dashboards.json"),
      "utf8",
    ),
  );
  const samples = Object.fromEntries(Object.entries(def.variables).map(([k, v]) => [k, v.sample]));
  const dashboards = load();
  let failures = 0;
  const bySlug = new Map();
  for (const d of dashboards) {
    const slug = d.uid.replace(/^qd-/, "").replace(/-[a-z]{2}$/, "");
    if (bySlug.has(slug) && bySlug.get(slug) !== queries(d)) {
      console.log(`[FAIL] ${d.uid}: its queries differ from another language's`);
      failures++;
    }
    bySlug.set(slug, queries(d));
    for (const v of d.templating.list) {
      const used = JSON.stringify([
        allPanels(d).map((p) => [p.targets ?? [], p.transformations ?? []]),
        d.templating.list.filter((w) => w !== v).map((w) => w.query),
      ]);
      const repeats = allPanels(d).some((p) => p.repeat === v.name);
      if (repeats || used.includes(`$${v.name}`) || used.includes(`\${${v.name}`)) continue;
      console.log(`[FAIL] ${d.uid}: no query uses the variable ${v.name}`);
      failures++;
    }
    for (const [name, values, only] of await cases(d, samples)) {
      const uses = (p) => p.repeat || followed(p).length;
      for (const p of allPanels(d).filter((x) => x.targets?.length && (!only || uses(x)))) {
        try {
          const rows = await runPanel(p, values);
          console.log(`[OK  ] ${d.uid} ${name}: ${p.title}: ${rows}`);
        } catch (e) {
          console.log(`[FAIL] ${d.uid} ${name}: ${p.title}: ${e.message}`);
          failures++;
        }
      }
    }
  }
  if (!dashboards.length) throw new Error(`no dashboards in ${dir}`);
  console.log(
    failures
      ? `${failures} query check(s) failed`
      : `all queries of ${dashboards.length} dashboards ran`,
  );
  return failures;
}

main().then(
  (failures) => process.exit(failures ? 1 : 0),
  (e) => {
    console.error(`check-queries: ${e.message}`);
    process.exit(1);
  },
);
