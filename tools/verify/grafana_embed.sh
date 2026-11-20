#!/bin/bash
# Verification V1 of decision 0073: qd-quality-<lang> in a frame of the
# application's page, as point 3 writes its URL, in this machine's own
# Chromium (headless, grafana_browser.py) against grafana_quality_lib.sh's
# stack: Traefik with the repository's middlewares (app-frame-headers on
# /grafana, security-headers on the page), the generated dashboards turned
# into Infinity queries (grafana_quality_dashboard.py), the API stand-in.
#
# It shows: the frame policy; the first load signed in to Quality Data but
# not to Grafana, and again; what kiosk, _dash.hideTimePicker and
# _dash.hideLinks leave; that Escape leaves kiosk; var-dataset hidden but
# set from the URL, var-cut taken even when not among the options, theme and
# the per-language uid; another dataset asked for in the URL (the API
# refuses it); panels below the fold queried only when scrolled to; two
# frames of two organisations side by side; and what the page sees with
# Grafana down. Only names, counts and timings are printed.
#
# Containers qd-verify-embed-…, network of that name, removed on exit, the
# browser stopped by its PID; no port published. Run it alone.
#
#   bash tools/verify/grafana_embed.sh
P=qd-verify-embed
NET_OPTS=--internal
. "$(dirname "$0")/machines_lib.sh"
. "$(dirname "$0")/grafana_quality_lib.sh"
start_run
infinity_plugin
quality_files
quality_dash en
quality_dash es
quality_stack || exit 1
# An organisation 1 dashboard, for the second frame.
mkdir -p "$D/dash/audit" && printf '{"uid":"qd-audit-en","title":"Audit trail","schemaVersion":42,"panels":[]}' >"$D/dash/audit/a.json"
printf '  - { name: audit, orgId: 1, type: file, options: { path: /dash/audit } }\n' >>"$D/prov/dashboards/quality.yml"
chmod -R a+rX "$D/dash" "$D/prov" && gfa grafana /api/admin/provisioning/dashboards/reload -X POST -o /dev/null

enc() { python3 -I -c 'import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1], safe=""))' "$1"; }
K="&kiosk&_dash.hideTimePicker&_dash.hideLinks"
frame() { echo "/grafana/d/qd-quality-$1/?orgId=2$K&var-dataset=$2&var-cut=$3&theme=$4"; }
page() { echo "https://$APP/page?src0=$(enc "$1")${2:+&src1=$(enc "$2")}"; }
get() { python3 -I -c 'import json,sys; v = json.load(sys.stdin)
for k in sys.argv[1].split("."): v = v[k] if not k.isdigit() else v[int(k)]
print(v if not isinstance(v, (list, dict)) else json.dumps(v))' "$1" <"$D/out.json"; }
api_since() { tail -n +"$(($1 + 1))" "$D/log/api.jsonl" | python3 -I -c 'import json,sys
print(" ".join(sorted(set(json.loads(l)["path"].split("/datasets/")[-1].split("/charts")[0] + "?" + json.loads(l)["query"] + "=" + str(json.loads(l)["status"]) for l in sys.stdin))))'; }
n_api() { if [ -f "$D/log/api.jsonl" ]; then wc -l <"$D/log/api.jsonl"; else echo 0; fi; }
IDS="[...new Set([...f.querySelectorAll('[data-testid]')].map(e=>e.getAttribute('data-testid')))]"
HAS="(t) => $IDS.some(x => x.startsWith(t))"
READY="f && f.querySelectorAll('[data-viz-panel-key]').length > 3"
SCROLL="(async()=>{for(let y=0;y<8000;y+=500){f.defaultView.scrollTo(0,y);[...f.querySelectorAll('div')].forEach(e=>{if(e.scrollHeight>e.clientHeight+100){e.scrollTop=y}});await new Promise(r=>setTimeout(r,250))}return true})()"

echo "--- the frame policy"
H=$(web ana "https://$APP/grafana/login" -D - -o /dev/null)
expect "Grafana's X-Frame-Options" SAMEORIGIN "$(echo "$H" | tr -d '\r' | sed -n 's/^x-frame-options: //Ip')"
expect "Grafana's frame-ancestors" "frame-ancestors 'self'" "$(echo "$H" | tr -d '\r' | sed -n 's/^content-security-policy: //Ip')"
expect "the page's CSP lets it frame its own origin" yes \
  "$(web ana "https://$APP/page" -D - -o /dev/null | grep -qi "^content-security-policy:.*frame-src 'self'" && echo yes || echo no)"

echo "--- ana, signed in to Quality Data, opens the page"
n0=$(n_api)
cat >"$D/s1.json" <<JSON
[{"mark":"m"},{"go":"https://$APP/api/test/signin?person=ana&back=$(enc "$(page "$(frame en loans 2026-09-30 light)")")"},
 {"wait":"$READY","timeout":90,"as":"t_cold"},{"sleep":5},{"net":"cold"},
 {"eval":"[($HAS)('data-testid Toggle menu'), ($HAS)('data-testid Nav toolbar'), ($HAS)('data-testid Home breadcrumb'), ($HAS)('data-testid Dashboard template variables')].join(' ')","as":"chrome"},
 {"eval":"[...f.querySelectorAll('label')].map(e=>e.textContent).join(' ')","as":"labels"},
 {"eval":"f.body.className.includes('theme-light') + ' ' + new URLSearchParams(f.location.search).get('var-dataset')","as":"theme_dataset"},
 {"mark":"m2"},{"eval":"$SCROLL","as":"scrolled"},{"sleep":5},{"net":"scrolled_net"},
 {"key":"Escape"},{"sleep":3},
 {"eval":"($HAS)('data-testid Toggle menu') + ' ' + new URLSearchParams(f.location.search).has('kiosk')","as":"escape"},
 {"mark":"m3"},{"go":"$(page "$(frame en loans 2026-09-30 light)")"},{"wait":"$READY","timeout":60,"as":"t_warm"},{"sleep":3},{"net":"warm"}]
JSON
PROFILE=$D/chrome-ana browse "$D/s1.json" >"$D/out.json"
expect "the frame reaches the dashboard, signing in to Grafana on its own" yes "$([ "$(get t_cold)" != None ] && echo yes || echo no)"
note "first load, Grafana not yet signed in: $(get t_cold) s to the panels; $(get cold.requests) requests, $(get cold.bytes) bytes, $(get cold.kinds)"
note "again, signed in: $(get t_warm) s; $(get warm.requests) requests, $(get warm.bytes) bytes"
expect "kiosk: menu, top bar, breadcrumb gone; variables bar kept" "false false false true" "$(get chrome)"
expect "the variables shown (dataset hidden)" "cut dimension drift_variable" "$(get labels)"
expect "theme light, dataset from the URL" "true loans" "$(get theme_dataset)"
expect "every chart asked for loans at the URL's cut" "" "$(tail -n +"$((n0 + 1))" "$D/log/api.jsonl" | python3 -I -c 'import json,sys
for l in sys.stdin:
    e = json.loads(l)
    if "/charts/" in e["path"] and ("/loans/" not in e["path"] or e["query"] != "cut=2026-09-30"): print(e["path"], e["query"])')"
note "charts queried before scrolling: $(get cold.kinds) → after scrolling the frame: $(get scrolled_net.kinds)"
expect "panels below the fold queried only once scrolled to" yes "$(python3 -I -c 'import json,sys; d = json.load(open(sys.argv[1])); print("yes" if d["scrolled_net"]["kinds"].get("ds/query", 0) > 0 else "no")' "$D/out.json")"
expect "Escape inside the frame leaves kiosk: menu shown, kiosk gone from the URL" "true false" "$(get escape)"

echo "--- URLs a person could make"
n0=$(n_api)
cat >"$D/s2.json" <<JSON
[{"go":"$(page "$(frame es loans 2025-01-31 dark)")"},{"wait":"$READY","timeout":60,"as":"t"},{"sleep":4},
 {"eval":"f.body.className.includes('theme-dark') + ' ' + f.body.innerText.includes('Conformidad e IGCD por corte')","as":"es"}]
JSON
PROFILE=$D/chrome-ana browse "$D/s2.json" >"$D/out.json"
expect "Spanish uid (its titles), dark theme" "true true" "$(get es)"
expect "a cut not among the variable's options reaches the API as given" yes \
  "$(api_since "$n0" | grep -q 'loans?cut=2025-01-31=200' && echo yes || echo no)"
n0=$(n_api)
cat >"$D/s3.json" <<JSON
[{"go":"$(page "$(frame en savings 2026-10-31 light)")"},{"wait":"$READY","timeout":60,"as":"t"},{"sleep":5},
 {"eval":"f.body.innerText.includes('Office 1')","as":"rows_shown"}]
JSON
PROFILE=$D/chrome-ana browse "$D/s3.json" >"$D/out.json"
expect "ana's frame set to savings: what the API answered" "savings?=403 savings?cut=2026-10-31=403" "$(api_since "$n0" | tr ' ' '\n' | grep -v '^/quality' | tr '\n' ' ' | sed 's/ $//')"
expect "and no row of savings drawn" False "$(get rows_shown)"

echo "--- carla, of both organisations: two frames side by side"
TWO=$(page "$(frame en loans 2026-10-31 light)" "/grafana/d/qd-audit-en/?orgId=1")
PATHS="f.location.pathname + ' | ' + document.querySelectorAll('iframe')[1].contentDocument.location.pathname"
cat >"$D/s4.json" <<JSON
[{"go":"https://$APP/api/test/signin?person=carla&back=$(enc "$(page "$(frame en loans 2026-10-31 light)")")"},
 {"wait":"$READY","timeout":90,"as":"t"},{"mark":"m"},{"go":"$TWO"},{"wait":"$READY","timeout":60,"as":"t2"},
 {"wait":"document.querySelectorAll('iframe')[1].contentDocument.location.pathname.includes('/d/qd-audit-en')","timeout":30,"as":"t1"},
 {"sleep":3},{"net":"two"},{"eval":"$PATHS","as":"paths"}]
JSON
PROFILE=$D/chrome-carla browse "$D/s4.json" >"$D/out.json"
expect "signed in to Grafana, each frame its own dashboard" "/grafana/d/qd-quality-en/quality-indicators | /grafana/d/qd-audit-en/audit-trail" "$(get paths)"
expect "organisations in X-Grafana-Org-Id across both frames' API calls" '["1", "2"]' "$(get two.api_orgs)"
# Both frames at once before Grafana has a session: two sign-ins race for
# Grafana's one OAuth state cookie. Recorded, not asserted: it is a race.
cat >"$D/s4b.json" <<JSON
[{"go":"https://$APP/api/test/signin?person=carla&back=$(enc "$TWO")"},{"sleep":20},{"eval":"$PATHS","as":"paths"}]
JSON
PROFILE=$D/chrome-carla2 browse "$D/s4b.json" >"$D/out.json"
note "two frames signing in at once, where each ended: $(get paths)"

echo "--- Grafana down"
docker stop "$P-grafana" >/dev/null
cat >"$D/s5.json" <<JSON
[{"go":"$(page "$(frame en loans 2026-10-31 light)")","timeout":30},{"sleep":3},
 {"eval":"f ? f.body.innerText.slice(0, 40) : 'unreadable'","as":"down"},
 {"eval":"fetch('/grafana/api/health').then(r => r.status)","as":"health"}]
JSON
browse "$D/s5.json" >"$D/out.json"
expect "the frame loads Traefik's page, readable by the page (same origin)" "Bad Gateway" "$(get down)"
expect "/grafana/api/health from the page" 502 "$(get health)"

echo
echo "passed $PASSED, failed $FAILED"
[ "$FAILED" = 0 ] && echo "ALL OK" || exit 1
