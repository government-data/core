#!/bin/bash
# Verification V6 of decision 0073: the requests and time of one page of
# qd-quality-en through Infinity, for each synthetic dataset, against the
# 18 chart requests the application's page makes today; and what Traefik's
# api-chain (the repository's middlewares: 50 requests a second per client
# address, bursts of 100) does when every query comes from Grafana's one
# address. grafana_quality_lib.sh's stack, the dashboard as step 6 would
# make it (grafana_quality_dashboard.py; point 4's hiding fails, V5),
# this machine's Chromium as carla, scrolled to the end so every panel runs.
# The stand-in's rows are generic, so the figures' transformations mostly
# draw "no data": request counts and times are what is measured, not drawing.
#
# Containers qd-verify-load-…, network of that name, removed on exit, the
# browser stopped by its PID; no port published. Run it alone.
#
#   bash tools/verify/grafana_dashboard_load.sh
P=qd-verify-load
NET_OPTS=--internal
. "$(dirname "$0")/machines_lib.sh"
. "$(dirname "$0")/grafana_quality_lib.sh"
start_run
infinity_plugin
quality_files
quality_dash en
quality_stack || exit 1

enc() { python3 -I -c 'import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1], safe=""))' "$1"; }
page() { echo "https://$APP/page?src0=$(enc "/grafana/d/qd-quality-en/?orgId=2&kiosk&_dash.hideTimePicker&_dash.hideLinks&var-dataset=$1&var-cut=2026-10-31&theme=light")"; }
get() { python3 -I -c 'import json,sys; v = json.load(sys.stdin)
for k in sys.argv[1].split("."): v = v.get(k, 0)
print(v if not isinstance(v, (list, dict)) else json.dumps(v))' "$1" <"$D/out.json"; }
count() { if [ -f "$1" ]; then wc -l <"$1"; else echo 0; fi; }
READY="f && f.querySelectorAll('[data-viz-panel-key]').length > 2"
SCROLL="(async()=>{for(let y=0;y<9000;y+=500){f.defaultView.scrollTo(0,y);[...f.querySelectorAll('div')].forEach(e=>{if(e.scrollHeight>e.clientHeight+100){e.scrollTop=y}});await new Promise(r=>setTimeout(r,200))}return true})()"
# Quiet: no panel still loading in the frame.
QUIET="f && f.querySelectorAll('[aria-label*=loading i], [data-testid*=loading i]').length === 0"

cat >"$D/s0.json" <<JSON
[{"go":"https://$APP/api/test/signin?person=carla&back=$(enc "$(page bare)")"},{"wait":"$READY","timeout":90,"as":"t"}]
JSON
PROFILE=$D/chrome-carla browse "$D/s0.json" >/dev/null
for ds in loans savings; do
  a=$(count "$D/log/api.jsonl")
  cat >"$D/s.json" <<JSON
[{"mark":"m"},{"go":"$(page $ds)"},{"wait":"$READY","timeout":60,"as":"t_first"},{"wait":"$QUIET","timeout":60,"as":"t_quiet"},
 {"net":"first"},{"eval":"$SCROLL","as":"s"},{"wait":"$QUIET","timeout":60,"as":"t_all"},{"sleep":2},{"net":"all"},
 {"eval":"f.querySelectorAll('[data-viz-panel-key]').length","as":"panels"}]
JSON
  PROFILE=$D/chrome-carla browse "$D/s.json" >"$D/out.json"
  calls=$(tail -n +"$((a + 1))" "$D/log/api.jsonl" | python3 -I -c 'import json,sys
c = [json.loads(l) for l in sys.stdin]
print(sum(1 for e in c if e.get("chart")), sum(1 for e in c if not e.get("chart")))')
  expect "$ds: the page loads and every panel finishes" yes "$(get t_all | grep -qE '^[0-9.]+$' && echo yes || echo no)"
  note "$ds: $(get panels) panels; first view $(get t_first) s to panels, quiet at $(get t_quiet) s with $(get first.kinds.ds/query) queries; scrolled to the end: $(get all.seconds) s, $(get all.requests) requests"
  note "$ds: at the API, ${calls% *} chart requests and ${calls#* } for the variables (the app's page: 18 charts and its list)"
  note "$ds: bytes to the browser for the page: $(get first.bytes) + $(get all.bytes)"
done

echo "--- every page's queries come from Grafana's one address"
QS=$(python3 -I -c 'import json,sys
c = ["trend_by_cut", "largest_groups", "groups_changed", "outliers_by_group", "remediation_queues", "groups_p_chart", "psi_by_decile", "sfc_view", "failures_by_dimension", "failures_by_field", "profiling", "conformity_by_rule", "time_by_stage"]
qs = [{"refId": f"Q{i}", "datasource": {"type": "yesoreyeram-infinity-datasource", "uid": "quality-api"}, "type": "json", "source": "url", "format": "table", "parser": "backend", "url": f"https://{sys.argv[1]}/api/quality/datasets/loans/charts/{c[i % len(c)]}?cut=2026-10-31", "url_options": {"method": "GET"}} for i in range(26)]
print(json.dumps({"from": "now-1h", "to": "now", "queries": qs}))' "$APP")
printf '%s' "$QS" >"$D/jar/qs.json" && chmod 644 "$D/jar/qs.json"
expect "carla signed in for curl too" "200 https://$APP/grafana/" "$(signin carla)"
t0=$(count "$D/log/access.json")
for pages in 1 6 20; do
  s0=$(date +%s.%N)
  docker run --rm --network "$NET" -v "$D/c:/c:ro" -v "$D/jar:/jar" --entrypoint sh "$CURL" -c '
    for i in $(seq '"$pages"'); do
      curl -s -m 60 --cacert /c/ca.crt -b /jar/carla -o /dev/null -w "grafana:%{http_code}\n" -X POST -H "Content-Type: application/json" -H "X-Grafana-Org-Id: 2" \
        --data @/jar/qs.json https://'"$APP"'/grafana/api/ds/query &
    done; wait' | sort | uniq -c | tr -s " " | tr "\n" ";"
  s1=$(date +%s.%N)
  sleep 2
  r=$(tail -n +"$((t0 + 1))" "$D/log/access.json" | python3 -I -c 'import json,sys
from collections import Counter
print(" ".join(f"{k}:{v}" for k, v in sorted(Counter(e["DownstreamStatus"] for e in map(json.loads, sys.stdin) if e.get("RouterName") == "api@file").items())))')
  t0=$(count "$D/log/access.json")
  note "$pages page(s) at once, 26 queries each, in $(python3 -I -c 'import sys; print(round(float(sys.argv[2]) - float(sys.argv[1]), 1))' "$s0" "$s1") s: Traefik's api router answered $r"
  [ "$pages" = 1 ] && expect "one page: every chart query within api-ratelimit" "200:26" "$r"
  # Six pages (156 queries in under 2 s) sit at the edge of 100 + 50/s:
  # all 200 in one run, one 429 in another. Recorded, not asserted.
  [ "$pages" = 20 ] && expect "20 pages at once (520 queries in about 2 s): api-ratelimit answers some with 429" yes \
    "$(echo "$r" | grep -q '429:' && echo yes || echo no)"
done

echo
echo "passed $PASSED, failed $FAILED"
[ "$FAILED" = 0 ] && echo "ALL OK" || exit 1
