// Writes the Grafana dashboards of decision 0026 (point 8) from their single
// definition, one folder per language:
//   node tools/grafana/dashboards.mjs        (pnpm run gen:grafana)
// Reads infra/observability/grafana/definition (dashboards.json, the SQL and
// PromQL in queries/<slug>/, the catalogues in locales/; the format is described in
// lib.mjs), the purposes of a view in openapi.json, and for the app's
// charts crates/quality/src/charts.rs, the SQL in crates/quality/charts/
// and the web's texts in packages/feature-quality/src/locales/
// (charts.mjs); it rewrites every subdirectory of
// infra/observability/grafana/dashboards, keeping its README.md. The output
// is deterministic, so `pnpm run check:grafana` can compare it with git.
// Directories 755 and files 644: Grafana (uid 472) reads them.

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { chartPanel, chartTables, parseCatalog, webTexts } from "./charts.mjs";
import { checkCatalogues, checkKeys, dashboard, references } from "./lib.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const grafana = path.join(root, "infra/observability/grafana");
const source = path.join(grafana, "definition");
const target = path.join(grafana, "dashboards");
const charts = path.join(root, "crates/quality/charts");

const readJson = (file) => JSON.parse(fs.readFileSync(file, "utf8"));

/**
 * The definition with each chart panel expanded from the app's catalogue
 * and SQL, failing when the catalogue and the SQL files disagree, and the
 * catalogues with the web's chart texts merged in.
 */
function expand(def, catalogues) {
  const catalog = parseCatalog(
    fs.readFileSync(path.join(root, "crates/quality/src/charts.rs"), "utf8"),
  );
  const files = fs.readdirSync(charts).filter((f) => f.endsWith(".sql"));
  const ids = catalog.map((c) => `${c.id}.sql`);
  const odd = [...files.filter((f) => !ids.includes(f)), ...ids.filter((f) => !files.includes(f))];
  if (odd.length)
    throw new Error(`charts.rs and crates/quality/charts disagree on ${odd.join(", ")}`);
  const web = Object.fromEntries(
    def.languages.map((lang) => [
      lang,
      webTexts(readJson(path.join(root, `packages/feature-quality/src/locales/${lang}.json`))),
    ]),
  );
  const sqlOf = (id) => fs.readFileSync(path.join(charts, `${id}.sql`), "utf8").trim();
  const ctx = { catalog, sqlOf, web: web[def.languages[0]], chartColumns: def.chartColumns };
  for (const d of def.dashboards) {
    const tables = d.tables && chartTables(d.panels, d.tables, ctx);
    d.panels = d.panels.map((p) => (p.chart ? chartPanel(p, ctx) : p));
    if (tables) d.panels.push(tables);
  }
  for (const lang of def.languages) Object.assign(catalogues[lang], web[lang]);
}

/**
 * Every column with `values: "purposes"` (decision 0069) maps a view's
 * purpose key to its text: each `purposes.<key>` of the catalogues, retired
 * ones too, and '' (a view audited before a purpose was asked) to
 * `personal-data.purpose_not_recorded`. Fails when a purpose of
 * openapi.json's ViewPurpose has no text.
 */
function purposes(def, catalogues) {
  const keys = readJson(path.join(root, "openapi.json")).components.schemas.ViewPurpose.enum;
  for (const [lang, catalogue] of Object.entries(catalogues)) {
    const missing = keys.filter((k) => typeof catalogue[`purposes.${k}`] !== "string");
    if (missing.length) throw new Error(`catalogue ${lang} has no text for purposes ${missing}`);
  }
  const prefix = "purposes.";
  const known = Object.keys(catalogues[def.languages[0]]).filter((k) => k.startsWith(prefix));
  const values = Object.fromEntries(known.map((k) => [k.slice(prefix.length), k]));
  values[""] = "personal-data.purpose_not_recorded";
  for (const d of def.dashboards) {
    for (const p of d.panels) {
      for (const spec of Object.values(p.columns ?? {})) {
        if (spec?.values === "purposes") spec.values = values;
      }
    }
  }
}

function load() {
  const def = readJson(path.join(source, "dashboards.json"));
  const catalogues = Object.fromEntries(
    def.languages.map((lang) => [lang, readJson(path.join(source, "locales", `${lang}.json`))]),
  );
  expand(def, catalogues);
  checkCatalogues(catalogues);
  purposes(def, catalogues);
  const { keys, files } = references(def);
  checkKeys(keys, catalogues);
  const sql = new Map();
  for (const file of files)
    sql.set(file, fs.readFileSync(path.join(source, "queries", file), "utf8").trim());
  const present = fs
    .readdirSync(path.join(source, "queries"), { recursive: true })
    .filter((f) => f.endsWith(".sql") || f.endsWith(".promql"))
    .map((f) => f.split(path.sep).join("/"));
  const unused = present.filter((f) => !files.has(f));
  if (unused.length) throw new Error(`query files no panel uses: ${unused.join(", ")}`);
  return { def, catalogues, sql };
}

/** The folder name must be a single path segment Grafana can show as is. */
function folderName(title) {
  if (!title || title.includes("/") || title.startsWith("."))
    throw new Error(`bad folder title: ${title}`);
  return title;
}

function main() {
  const { def, catalogues, sql } = load();
  const output = new Map();
  for (const lang of def.languages) {
    const t = (key) => catalogues[lang][key];
    for (const d of def.dashboards) {
      const folder = folderName(t(d.folder ?? "folder"));
      const json = dashboard(d, lang, { def, t, sqlOf: (f) => sql.get(f) });
      output.set(path.join(folder, `${d.slug}.json`), `${JSON.stringify(json, null, 2)}\n`);
    }
  }
  for (const entry of fs.readdirSync(target, { withFileTypes: true })) {
    if (entry.isDirectory()) fs.rmSync(path.join(target, entry.name), { recursive: true });
  }
  for (const [file, text] of [...output].sort(([a], [b]) => (a < b ? -1 : 1))) {
    const dir = path.join(target, path.dirname(file));
    fs.mkdirSync(dir, { recursive: true });
    fs.chmodSync(dir, 0o755);
    fs.writeFileSync(path.join(target, file), text);
    fs.chmodSync(path.join(target, file), 0o644);
    console.log(`wrote ${path.relative(root, path.join(target, file))}`);
  }
}

try {
  main();
} catch (e) {
  console.error(`grafana dashboards: ${e.message}`);
  process.exit(1);
}
