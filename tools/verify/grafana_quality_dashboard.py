"""The generated qd-quality-<lang> dashboard turned, for decision 0073's
verification only, into what its step 6 makes of it: every chart panel an
Infinity query of the API's chart route, the variables read from the API,
`dataset` hidden. The chart of each query is found by its SQL's first line
(the generator embeds crates/quality/charts/<id>.sql). Standard library.

    python3 -I grafana_quality_dashboard.py GENERATED.json CHARTS_DIR BASE_URL OUT.json [--meets]

--meets adds what point 4 proposes to hide the charts a dataset does not
meet: a hidden variable per chart with needs, read from the API's `meets`,
and its panel repeated by it (V5).
"""

import glob
import json
import os
import sys

src, charts_dir, base, out = sys.argv[1:5]
MEETS = "--meets" in sys.argv
DS = {"type": "yesoreyeram-infinity-datasource", "uid": "quality-api"}
NEEDS = ("groups_p_chart", "largest_groups", "groups_changed", "outliers_by_group", "psi_by_decile",
         "remediation_queues", "icd_distribution", "bands_by_segment", "band_transitions", "lowest_icd",
         "sfc_view", "update_recency")
first = {os.path.basename(f)[:-4]: open(f).read().strip().splitlines()[0]
         for f in glob.glob(os.path.join(charts_dir, "*.sql"))}
d = json.load(open(src))


def chart_of(sql):
    found = [c for c, line in first.items() if line in sql]
    if len(found) != 1:
        raise SystemExit(f"no single chart for a query: {found}")
    return found[0]


def infinity(url, selector="", columns=()):
    return {"type": "json", "source": "url", "format": "table", "parser": "backend", "url": base + url,
            "url_options": {"method": "GET"}, "root_selector": selector,
            "columns": [{"selector": c, "text": c, "type": "string"} for c in columns]}


def variable(name, url, selector, column, hide=0, multi=False):
    return {"name": name, "label": name, "type": "query", "datasource": DS, "hide": hide, "refresh": 1,
            "multi": multi, "includeAll": multi, "sort": 0, "regex": "", "options": [],
            "current": {"text": ["All"], "value": ["$__all"]} if multi else {},
            "query": {"queryType": "infinity", "refId": name, "infinityQuery": infinity(url, selector, [column])}}


charts = set()


def convert(panels):
    for p in panels:
        for t in p.get("targets", []):
            chart = chart_of(t["rawSql"])
            charts.add(chart)
            t.clear()
            t.update(refId="A", datasource=DS, **infinity(f"datasets/${{dataset}}/charts/{chart}?cut=${{cut}}"))
        if p.get("targets"):
            p["datasource"] = DS
        convert(p.get("panels", []))
        chart = chart_of_panel(p) if p.get("targets") else None
        if MEETS and chart in NEEDS and not p.get("repeat") and p["type"] != "table":
            p["repeat"], p["repeatDirection"] = f"meets_{chart}", "h"


def chart_of_panel(p):
    return p["targets"][0]["url"].split("/charts/")[1].split("?")[0]


convert(d["panels"])
v = [variable("dataset", "datasets", "", "dataset", hide=2),
     variable("cut", "datasets/${dataset}/charts", "cuts", "cut"),
     variable("dimension", "datasets/${dataset}/charts", "dimensions", "dimension", multi=True),
     variable("drift_variable", "datasets/${dataset}/charts", "drift_variables", "variable", multi=True)]
v[1]["query"]["infinityQuery"]["columns"] = []  # a list of strings
if MEETS:
    for c in sorted(set(NEEDS) & charts):
        m = variable(f"meets_{c}", "datasets/${dataset}/charts", "meets", "chart", hide=2)
        m["regex"] = f"/^{c}$/"
        v.append(m)
d["templating"]["list"] = v
json.dump(d, open(out, "w"))
print(len(charts), "charts;", sum(1 for p in d["panels"] if p.get("repeat")), "repeated panels")
