#!/bin/bash
# Verification V5 of decision 0073: a panel repeated by a query variable
# that has no values, in Grafana 13.2.5 (schema version 42), with Infinity
# as the variable's source; the way point 4 proposes to hide the charts a
# dataset does not meet. grafana_quality_dashboard.py --meets gives each
# chart with needs that is not already repeated a hidden variable
# meets_<chart> (the API's `meets` list filtered by a regex to that one
# chart) and repeats the panel by it; the group charts repeat by
# `dimension` and PSI by `drift_variable`, as generated. The stand-in's
# datasets: loans meets the SFC view and has two dimensions and a drift
# variable, savings meets update recency and has three dimensions and no
# drift variable, bare meets neither and has no dimension at all.
#
# It shows, in this machine's Chromium as carla (who may view all three):
# which panels are drawn for each dataset, once scrolled to the end, and
# which charts the API was asked for. Only panel keys and counts are printed.
#
# Containers qd-verify-repeat-…, network of that name, removed on exit, the
# browser stopped by its PID; no port published. Run it alone.
#
#   bash tools/verify/grafana_repeat_empty.sh
P=qd-verify-repeat
NET_OPTS=--internal
. "$(dirname "$0")/machines_lib.sh"
. "$(dirname "$0")/grafana_quality_lib.sh"
start_run
infinity_plugin
quality_files
quality_dash en --meets
quality_stack || exit 1
expect "the dashboard's schema version" 42 "$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["schemaVersion"])' "$D/dash/quality/en.json")"

enc() { python3 -I -c 'import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1], safe=""))' "$1"; }
page() { echo "https://$APP/page?src0=$(enc "/grafana/d/qd-quality-en/?orgId=2&kiosk&var-dataset=$1&var-cut=2026-10-31&theme=light")"; }
READY="f && f.querySelectorAll('[data-viz-panel-key]').length > 2"
SCROLL="(async()=>{for(let y=0;y<9000;y+=500){f.defaultView.scrollTo(0,y);[...f.querySelectorAll('div')].forEach(e=>{if(e.scrollHeight>e.clientHeight+100){e.scrollTop=y}});await new Promise(r=>setTimeout(r,250))}return true})()"
KEYS="[...f.querySelectorAll('[data-viz-panel-key]')].map(e=>e.getAttribute('data-viz-panel-key').replace('panel-','')).join(' ')"
# Panels: 4 lowest ICD, 5 ICD distribution, 6 bands, 7 transitions, 8 SFC,
# 15 recency (repeated by meets_*); 9 to 12 and 14 by dimension; 13 PSI by
# drift_variable; 1, 2, 3, 16, 17, 18 not repeated.
first=yes
# What 13.2.5 does (the record hoped for zero): a repeat with no values
# draws its panel once, with an empty value, and queries it.
for t in "loans|1 2 3 4 5 6 7 8 9 9-clone-1 10 10-clone-1 11 11-clone-1 12 12-clone-1 13 14 14-clone-1 15 16 17 18" \
  "savings|1 2 3 4 5 6 7 8 9 9-clone-1 9-clone-2 10 10-clone-1 10-clone-2 11 11-clone-1 11-clone-2 12 12-clone-1 12-clone-2 13 14 14-clone-1 14-clone-2 15 16 17 18" \
  "bare|1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18"; do
  ds=${t%%|*}
  n=$(if [ -f "$D/log/api.jsonl" ]; then wc -l <"$D/log/api.jsonl"; else echo 0; fi)
  go="$(page "$ds")"
  [ $first = yes ] && go="https://$APP/api/test/signin?person=carla&back=$(enc "$go")" && first=no
  cat >"$D/s.json" <<JSON
[{"go":"$go"},{"wait":"$READY","timeout":90,"as":"t"},{"sleep":4},{"eval":"$SCROLL","as":"s"},{"sleep":5},
 {"eval":"$KEYS","as":"keys"}]
JSON
  PROFILE=$D/chrome-carla browse "$D/s.json" >"$D/out.json"
  keys=$(python3 -I -c 'import json,sys; print(" ".join(sorted(json.load(open(sys.argv[1]))["keys"].split(), key=lambda k: (int(k.split("-")[0]), k))))' "$D/out.json")
  expect "$ds: panels drawn" "${t#*|}" "$keys"
  note "$ds: charts the API was asked for: $(tail -n +"$((n + 1))" "$D/log/api.jsonl" | python3 -I -c 'import json,sys; print(*sorted({json.loads(l).get("chart", "") for l in sys.stdin} - {""}))')"
done
expect "charts the dataset does not meet are asked of the API all the same" yes \
  "$([ "$(api_log '(e.get("chart") in ("icd_distribution", "bands_by_segment", "band_transitions", "lowest_icd")) or (e.get("chart") == "sfc_view" and "/loans/" not in e["path"]) or (e.get("chart") == "update_recency" and "/savings/" not in e["path"])' | wc -l)" -gt 0 ] && echo yes || echo no)"
# Why, in the 13.2.5 bundle: DashboardGridItem.performRepeat falls back to
# one value "" (text "None", or "All") when the variable has none.
expect "the bundle's repeat falls back to one empty value" yes \
  "$(docker exec "$P-grafana" sh -c 'grep -lq "values:\[\"\"\],texts:[A-Za-z_]*.hasAllValue()?\[\"All\"\]:\[\"None\"\]" public/build/*.js' && echo yes || echo no)"

echo
echo "passed $PASSED, failed $FAILED"
[ "$FAILED" = 0 ] && echo "ALL OK" || exit 1
