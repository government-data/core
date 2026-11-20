#!/usr/bin/env node
// The HTML report of an interactive test round: the inventory's cases (a
// markdown file) with what the round wrote in its results.json. One file,
// screenshots inside, that opens by itself.
//
//   node tools/qa/report.mjs \
//     --inventory qa/testing-inventory.md \
//     --results qa/<round>/results.json --out qa/<round>/report.html

import fs from "node:fs";
import path from "node:path";

// `--name value`, or `--flag` alone (true).
const argv = process.argv.slice(2);
const args = Object.fromEntries(
  argv.reduce(
    (pairs, a, i) =>
      a.startsWith("--")
        ? [
            ...pairs,
            [a.slice(2), argv[i + 1] && !argv[i + 1].startsWith("--") ? argv[i + 1] : true],
          ]
        : pairs,
    [],
  ),
);
for (const need of ["inventory", "results", "out"]) {
  if (!args[need]) {
    console.error(`missing --${need}`);
    process.exit(2);
  }
}

/** The inventory's modules and cases, from its markdown tables. */
function readInventory(file) {
  const modules = [];
  let current = null;
  let columns = null;
  for (const line of fs.readFileSync(file, "utf8").split("\n")) {
    const heading = /^## (.+?)\s*$/.exec(line);
    if (heading) {
      current = { title: heading[1], cases: [] };
      modules.push(current);
      columns = null;
      continue;
    }
    if (!line.startsWith("|") || !current) continue;
    const cells = line
      .slice(1, line.trimEnd().endsWith("|") ? line.trimEnd().length - 1 : undefined)
      .split("|")
      .map((c) => c.trim());
    if (cells[0] === "ID") {
      columns = cells;
      continue;
    }
    if (!columns || !/^[A-Z]+-\d+$/.test(cells[0])) continue;
    const row = Object.fromEntries(columns.map((c, i) => [c, cells[i] ?? ""]));
    current.cases.push({
      id: row["ID"],
      priority: row["Priority"],
      what: row["What is tested"],
      steps: row["Steps"],
      expected: row["Expected result"],
      requires: row["Requires"],
    });
  }
  return modules.filter((m) => m.cases.length > 0);
}

const STATES = {
  passed: { label: "Passed", tone: "ok" },
  failed: { label: "Failed", tone: "bad" },
  blocked: { label: "Blocked", tone: "warn" },
  skipped: { label: "Skipped", tone: "muted" },
  untested: { label: "Untested", tone: "muted" },
};

const esc = (s) =>
  String(s ?? "").replace(
    /[&<>"']/g,
    (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c],
  );
/** The inventory's inline markdown that matters here: `code`. */
const md = (s) => esc(s).replace(/`([^`]+)`/g, "<code>$1</code>");

const inventory = readInventory(args.inventory);
const results = JSON.parse(fs.readFileSync(args.results, "utf8"));
const baseDir = path.dirname(args.results);
const byId = new Map((results.cases ?? []).map((c) => [c.id, c]));
const known = new Set(inventory.flatMap((m) => m.cases.map((c) => c.id)));
const unknown = (results.cases ?? []).filter((c) => !known.has(c.id)).map((c) => c.id);

/** A screenshot inside the page (data URI), or a note if it is missing. */
function image(rel) {
  const file = path.resolve(baseDir, rel);
  if (!fs.existsSync(file)) return `<p class="missing">Missing screenshot ${esc(rel)}</p>`;
  const ext = path.extname(file).slice(1).toLowerCase();
  const mime = ext === "jpg" || ext === "jpeg" ? "image/jpeg" : `image/${ext}`;
  const data = fs.readFileSync(file).toString("base64");
  return `<a href="data:${mime};base64,${data}" target="_blank" rel="noopener"><img loading="lazy" alt="${esc(rel)}" src="data:${mime};base64,${data}"></a>`;
}

const all = inventory.flatMap((m) =>
  m.cases.map((c) => ({ ...c, module: m.title, result: byId.get(c.id) })),
);
const stateOf = (c) => (c.result?.state && STATES[c.result.state] ? c.result.state : "untested");
const count = (s) => all.filter((c) => stateOf(c) === s).length;
const tested = count("passed") + count("failed");
const passRate = tested ? Math.round((count("passed") / tested) * 100) : 0;
const findings = all
  .filter((c) => stateOf(c) === "failed")
  .sort((a, b) => a.priority.localeCompare(b.priority) || a.id.localeCompare(b.id));
const blockedA = all.filter((c) => c.priority === "A" && stateOf(c) !== "passed");

function caseCard(c) {
  const s = stateOf(c);
  const r = c.result ?? {};
  return `<article class="case" data-state="${esc(s)}" id="${esc(c.id)}">
  <header>
    <span class="id">${esc(c.id)}</span>
    <span class="prio prio-${esc(c.priority)}" title="Priority">${esc(c.priority)}</span>
    <h3>${md(c.what)}</h3>
    <span class="state ${STATES[s].tone}">${STATES[s].label}</span>
  </header>
  ${r.finding ? `<p class="finding">${md(r.finding)}</p>` : ""}
  ${r.notes ? `<p class="notes">${md(r.notes)}</p>` : ""}
  <details>
    <summary>Steps and expected result</summary>
    <dl>
      <dt>Steps</dt><dd>${md(c.steps)}</dd>
      <dt>Expected</dt><dd>${md(c.expected)}</dd>
      ${c.requires && c.requires !== "—" ? `<dt>Requires</dt><dd>${md(c.requires)}</dd>` : ""}
    </dl>
  </details>
  ${(r.evidence ?? []).length ? `<div class="shots">${r.evidence.map(image).join("")}</div>` : ""}
</article>`;
}

const round = results.round ?? {};
const html = `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Test report</title>
${args.live ? '<meta http-equiv="refresh" content="5">' : ""}
<style>
:root {
  --bg: #f4f5f7; --surface: #fff; --text: #1d2433; --muted: #667085; --border: #e3e6eb;
  --ok: #157f3b; --ok-bg: #e5f6ea; --bad: #c4321c; --bad-bg: #fdecea; --warn: #9a6200; --warn-bg: #fff4dc;
  --accent: #4c6ef5; --code: #eef1f6;
}
@media (prefers-color-scheme: dark) {
  :root:not([data-theme="light"]) {
    --bg: #11151c; --surface: #1a2029; --text: #e6e9ef; --muted: #9aa4b2; --border: #2b3440;
    --ok: #5fd38a; --ok-bg: #173323; --bad: #ff8a7a; --bad-bg: #3a1d1a; --warn: #f5c35c; --warn-bg: #3a2f14;
    --accent: #8ea6ff; --code: #242c38;
  }
}
:root[data-theme="dark"] {
  --bg: #11151c; --surface: #1a2029; --text: #e6e9ef; --muted: #9aa4b2; --border: #2b3440;
  --ok: #5fd38a; --ok-bg: #173323; --bad: #ff8a7a; --bad-bg: #3a1d1a; --warn: #f5c35c; --warn-bg: #3a2f14;
  --accent: #8ea6ff; --code: #242c38;
}
* { box-sizing: border-box; }
body { margin: 0; background: var(--bg); color: var(--text); font: 15px/1.5 system-ui, -apple-system, "Segoe UI", Roboto, sans-serif; }
main { max-width: 1100px; margin: 0 auto; padding: 24px 16px 64px; }
h1 { margin: 0 0 4px; font-size: 1.6rem; }
h2 { margin: 32px 0 12px; font-size: 1.15rem; }
h3 { margin: 0; font-size: 1rem; flex: 1 1 12rem; }
.meta { color: var(--muted); margin: 0 0 20px; }
.cards { display: grid; grid-template-columns: repeat(auto-fit, minmax(140px, 1fr)); gap: 12px; }
.card { background: var(--surface); border: 1px solid var(--border); border-radius: 10px; padding: 14px; }
.card b { display: block; font-size: 1.6rem; }
.card span { color: var(--muted); font-size: .85rem; }
.card.ok b { color: var(--ok); } .card.bad b { color: var(--bad); } .card.warn b { color: var(--warn); }
.alert { background: var(--bad-bg); color: var(--bad); border-radius: 10px; padding: 12px 14px; margin-top: 16px; }
a { color: var(--accent); }
.alert a { color: inherit; }
.filters { display: flex; flex-wrap: wrap; gap: 8px; margin: 20px 0 0; }
.filters button { font: inherit; border: 1px solid var(--border); background: var(--surface); color: var(--text); border-radius: 999px; padding: 4px 12px; cursor: pointer; }
.filters button[aria-pressed="true"] { border-color: var(--accent); color: var(--accent); }
.case { background: var(--surface); border: 1px solid var(--border); border-radius: 10px; padding: 12px 14px; margin-bottom: 10px; }
.case header { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; }
.id { font: 600 .8rem ui-monospace, monospace; color: var(--muted); }
.prio { font-size: .72rem; font-weight: 700; border: 1px solid var(--border); border-radius: 4px; padding: 0 5px; }
.prio-A { color: var(--bad); border-color: currentColor; }
.state { font-size: .8rem; font-weight: 600; border-radius: 999px; padding: 2px 10px; }
.state.ok { background: var(--ok-bg); color: var(--ok); }
.state.bad { background: var(--bad-bg); color: var(--bad); }
.state.warn { background: var(--warn-bg); color: var(--warn); }
.state.muted { background: var(--code); color: var(--muted); }
.finding { background: var(--bad-bg); color: var(--bad); border-radius: 8px; padding: 8px 10px; margin: 10px 0 0; }
.notes { margin: 8px 0 0; }
details { margin-top: 8px; color: var(--muted); }
summary { cursor: pointer; font-size: .85rem; }
dl { display: grid; grid-template-columns: max-content 1fr; gap: 4px 12px; margin: 8px 0 0; }
dt { font-weight: 600; } dd { margin: 0; color: var(--text); }
code { background: var(--code); border-radius: 4px; padding: 0 4px; font-size: .88em; }
.shots { display: flex; flex-wrap: wrap; gap: 8px; margin-top: 10px; }
.shots img { max-width: 100%; width: 320px; border: 1px solid var(--border); border-radius: 6px; display: block; }
.missing { color: var(--warn); font-size: .85rem; }
.hidden { display: none; }
.live { display: inline-block; background: var(--warn-bg); color: var(--warn); border-radius: 999px; padding: 2px 12px; margin: 0 0 8px; font-size: .85rem; }
@media (max-width: 520px) { .shots img { width: 100%; } dl { grid-template-columns: 1fr; } }
</style>
</head>
<body>
<main>
<h1>Test report</h1>
${args.live ? '<p class="live">Round in progress: this page reloads itself every 5 seconds.</p>' : ""}
<p class="meta">${esc(round.environment ?? "")}${round.version ? ` · ${esc(round.version)}` : ""}${round.date ? ` · ${esc(new Date(round.date).toLocaleString())}` : ""}${round.tester ? ` · ${esc(round.tester)}` : ""}</p>

<section class="cards" aria-label="Summary">
  <div class="card"><b>${all.length}</b><span>cases in the inventory</span></div>
  <div class="card ok"><b>${count("passed")}</b><span>passed</span></div>
  <div class="card bad"><b>${count("failed")}</b><span>failed</span></div>
  <div class="card warn"><b>${count("blocked")}</b><span>blocked</span></div>
  <div class="card"><b>${count("skipped") + count("untested")}</b><span>skipped or untested</span></div>
  <div class="card"><b>${passRate}%</b><span>of what was tested passed</span></div>
</section>
${blockedA.length ? `<p class="alert">${blockedA.length} priority A case(s) did not pass: ${blockedA.map((c) => `<a href="#${esc(c.id)}">${esc(c.id)}</a>`).join(", ")}.</p>` : ""}
${unknown.length ? `<p class="alert">Results with an ID that is not in the inventory: ${unknown.map(esc).join(", ")}.</p>` : ""}

${findings.length ? `<h2>Findings</h2>${findings.map(caseCard).join("\n")}` : `<h2>Findings</h2><p class="meta">No case failed.</p>`}

<div class="filters" role="group" aria-label="Filter by state">
  <button type="button" data-filter="" aria-pressed="true">All</button>
  ${Object.entries(STATES)
    .map(
      ([k, v]) =>
        `<button type="button" data-filter="${esc(k)}" aria-pressed="false">${v.label} (${count(k)})</button>`,
    )
    .join("")}
</div>

${inventory
  .map(
    (m) => `<section class="module">
<h2>${esc(m.title)}</h2>
${all
  .filter((c) => c.module === m.title)
  .map(caseCard)
  .join("\n")}
</section>`,
  )
  .join("\n")}
</main>
<script>
document.querySelectorAll(".filters button").forEach((b) =>
  b.addEventListener("click", () => {
    const f = b.dataset.filter;
    document.querySelectorAll(".filters button").forEach((x) => x.setAttribute("aria-pressed", String(x === b)));
    document.querySelectorAll(".module .case").forEach((c) => c.classList.toggle("hidden", !!f && c.dataset.state !== f));
    document.querySelectorAll(".module").forEach((m) =>
      m.classList.toggle("hidden", !m.querySelector(".case:not(.hidden)")),
    );
  }),
);
</script>
</body>
</html>
`;

fs.writeFileSync(args.out, html);
console.log(
  `report: ${args.out} · ${count("passed")} passed, ${count("failed")} failed, ${count("blocked")} blocked, ${count("skipped") + count("untested")} untested`,
);
