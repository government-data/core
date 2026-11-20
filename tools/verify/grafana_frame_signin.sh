#!/bin/bash
# Verification V2 of decision 0073: signing in to Grafana inside the
# application's frame, and what happens as the provider's token and
# Grafana's session age. grafana_quality_lib.sh's stack, this machine's
# Chromium for the frame, curl for the timed parts; the provider stand-in
# issues no refresh token, as Quality Data's (0046), its tokens shortened
# to 60 s here, and Grafana's login_maximum_lifetime_duration to 2 minutes
# in the second half (the record sets 24 h; the behaviour, not the length,
# is what is shown).
#
# It shows: the frame's silent sign-in with Quality Data's session (the
# SameSite=Strict cookie sent along the frame's same-site redirects) and the
# cookies Grafana sets; the frame without that session; a query after the
# token expired; Grafana's session past its maximum lifetime; Quality
# Data's sign-out; a permission withdrawn. Only names, statuses, verdicts
# and timings are printed.
#
# Containers qd-verify-signin-…, network and volume of that name, removed on
# exit, the browser stopped by its PID; no port published. Run it alone.
#
#   bash tools/verify/grafana_frame_signin.sh
P=qd-verify-signin
NET_OPTS=--internal
TOKEN_TTL=60
. "$(dirname "$0")/machines_lib.sh"
. "$(dirname "$0")/grafana_quality_lib.sh"
start_run
infinity_plugin
quality_files
quality_dash en
GF_VOLUME_OPTS=(-v "$P-grafana-data:/var/lib/grafana")
quality_stack "${GF_VOLUME_OPTS[@]}" || exit 1

enc() { python3 -I -c 'import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1], safe=""))' "$1"; }
FRAME="/grafana/d/qd-quality-en/?orgId=2&kiosk&_dash.hideTimePicker&_dash.hideLinks&var-dataset=loans&var-cut=2026-10-31&theme=light"
PAGE="https://$APP/page?src0=$(enc "$FRAME")"
get() { python3 -I -c 'import json,sys; v = json.load(sys.stdin)[sys.argv[1]]; print(v if not isinstance(v, (list, dict)) else json.dumps(v))' "$1" <"$D/out.json"; }
READY="f && f.querySelectorAll('[data-viz-panel-key]').length > 3"
q='[{"refId":"A","datasource":{"type":"yesoreyeram-infinity-datasource","uid":"quality-api"},"type":"json","source":"url","format":"table","parser":"backend","url":"https://'$APP'/api/quality/datasets/loans/charts/trend_by_cut","url_options":{"method":"GET"}}]'
# query PERSON: the HTTP status, the panel's error if any, and what the API
# said of the token that came.
query() {
  local n
  n=$(if [ -f "$D/log/api.jsonl" ]; then wc -l <"$D/log/api.jsonl"; else echo 0; fi)
  ds_query "$1" 2 "$q" -w '\n%{http_code}' | python3 -I -c 'import json,sys
body, code = sys.stdin.read().rsplit("\n", 1)
try: r = json.loads(body); e = r["results"]["A"].get("error", "") if "results" in r else r.get("message", "")
except Exception: e = body[:60]
print(code, ("error: " + e[:60]) if e else "data")'
  tail -n +"$((n + 1))" "$D/log/api.jsonl" 2>/dev/null | python3 -I -c 'import json,sys; print("api:", *[json.loads(l)["auth"] + "/" + str(json.loads(l)["status"]) for l in sys.stdin] or ["nothing"])'
}
status() { as "$1" /api/user -o /dev/null -w '%{http_code}'; }

echo "--- the frame signs in to Grafana on its own"
cat >"$D/s1.json" <<JSON
[{"go":"https://$APP/api/test/signin?person=ana&back=$(enc "$PAGE")"},{"wait":"$READY","timeout":90,"as":"t"},
 {"eval":"location.pathname + ' ' + f.location.pathname","as":"where"},{"cookies":"cookies"}]
JSON
PROFILE=$D/chrome-ana browse "$D/s1.json" >"$D/out.json"
expect "the page stays, the frame reaches the dashboard" "/page /grafana/d/qd-quality-en/quality-indicators" "$(get where)"
note "seconds to the panels, Grafana's sign-in included: $(get t)"
expect "the provider's authorize got Quality Data's session cookie from the frame" True \
  "$(api_log 'e["path"] == "/oauth/authorize"' app | tail -1 | python3 -I -c 'import json,sys; print(json.loads(sys.stdin.readline())["session"])')"
expect "the cookies, by name and attributes" '["grafana_session path=/grafana secure=True httponly=True samesite=Lax", "grafana_session_expiry path=/grafana secure=True httponly=False samesite=Lax", "qd_session path=/ secure=True httponly=True samesite=Strict"]' "$(get cookies)"

echo "--- the frame without Quality Data's session"
cat >"$D/s2.json" <<JSON
[{"go":"$PAGE"},{"sleep":10},
 {"eval":"location.pathname + ' ' + (f ? f.location.pathname : 'unreadable')","as":"where"}]
JSON
browse "$D/s2.json" >"$D/out.json"
expect "the page stays; the frame's document cannot be read (the browser's error page)" "/page unreadable" "$(get where)"
expect "the frame was sent to the sign-in page" yes "$(api_log 'e["path"] == "/signin"' app | grep -q . && echo yes || echo no)"
expect "which refuses frames (security-headers)" "DENY" \
  "$(web nobody "https://$APP/signin" -D - -o /dev/null | tr -d '\r' | sed -n 's/^x-frame-options: //Ip')"

echo "--- the provider's token expires (60 s here, no refresh token)"
signin ana >/dev/null
expect "a query with a live token" "200 data api: ok:ana/200" "$(query ana | tr '\n' ' ' | sed 's/ $//')"
sleep 70
R=$(query ana | tr '\n' ' ' | sed 's/ $//')
echo "       after 70 s: $R"
note "Grafana's session after the token expired: /api/user $(status ana)"
expect "Grafana forwards the expired token; the API refuses it" "api: expired/401" "${R##*error*HTTP }"
note "Grafana's log on it: $(docker logs "$P-grafana" 2>&1 | grep -i 'expired\|refresh' | grep -io 'msg="[^"]*"' | sort | uniq -c | tr '\n' ';')"
cat >"$D/s3.json" <<JSON
[{"go":"$PAGE"},{"sleep":2},{"wait":"$READY","timeout":60,"as":"t"},{"sleep":3},{"eval":"f.location.pathname","as":"where"}]
JSON
n=$(api_log 'e["path"] == "/oauth/token"' app | wc -l)
a=$(if [ -f "$D/log/api.jsonl" ]; then wc -l <"$D/log/api.jsonl"; else echo 0; fi)
PROFILE=$D/chrome-ana browse "$D/s3.json" >"$D/out.json"
expect "the frame reopened after the token expired still shows the dashboard" "/grafana/d/qd-quality-en/quality-indicators" "$(get where)"
expect "with no new sign-in: tokens issued meanwhile" 0 $(($(api_log 'e["path"] == "/oauth/token"' app | wc -l) - n))
expect "and every query of its panels carried the expired token" "expired" \
  "$(tail -n +"$((a + 1))" "$D/log/api.jsonl" | python3 -I -c 'import json,sys; print(*sorted({json.loads(l)["auth"] for l in sys.stdin}))')"

echo "--- Grafana's session past login_maximum_lifetime_duration (2 m here)"
docker rm -f "$P-app" "$P-grafana" >/dev/null
TOKEN_TTL=86400
docker run -d --name "$P-app" --network "$NET" --network-alias app -v "$D/c:/c:ro" -v "$D/log:/log" \
  -e KEY_FILE=/c/jwt.key -e PEOPLE_FILE=/c/people.json -e LOG_FILE=/log/app.jsonl -e TOKEN_TTL=86400 \
  -e CLIENT_SECRET_FILE=/c/client_secret -e REDIRECT_URI="https://$APP/grafana/login/generic_oauth" \
  "$PY_IMAGE" python3 -I /c/standins.py app >/dev/null
GF_MAX_LIFETIME=2m quality_grafana_up grafana "${GF_VOLUME_OPTS[@]}" || exit 1
rm -f "$D/jar/ana"
signin ana >/dev/null
expect "a fresh session" 200 "$(status ana)"
sleep 130
expect "the session after 130 s" 401 "$(status ana)"
cat >"$D/s3b.json" <<JSON
[{"go":"$PAGE"},{"sleep":2},{"wait":"$READY","timeout":40,"as":"t"},{"sleep":3},
 {"eval":"f ? f.location.pathname : 'unreadable'","as":"where"},{"eval":"f ? f.body.innerText.slice(0, 160) : ''","as":"text"}]
JSON
PROFILE=$D/chrome-ana browse "$D/s3b.json" >"$D/out.json"
note "the frame reopened with the session past its lifetime: $(get where) — $(get text | tr '\n' ' ')"
expect "the frame reopened then signs in again on its own" "/grafana/d/qd-quality-en/quality-indicators" "$(get where)"

echo "--- signing out of Quality Data, then a permission withdrawn"
rm -f "$D/jar/ana"
signin ana >/dev/null
web ana "https://$APP/api/test/signout" -o /dev/null
expect "Grafana's session after Quality Data's sign-out" 200 "$(status ana)"
expect "and its queries still carry a live token" "200 data api: ok:ana/200" "$(query ana | tr '\n' ' ' | sed 's/ $//')"
python3 -I -c 'import json,sys; p=json.load(open(sys.argv[1])); p["ana"]["datasets"]=[]; json.dump(p, open(sys.argv[1], "w"))' "$D/c/people.json"
R=$(query ana | tr '\n' ' ' | sed 's/ $//')
case "$R" in *"api: ok:ana/403") ok "loans withdrawn from ana: the next query refused at once ($R)" ;; *) ko "withdrawn: $R" ;; esac

echo
echo "passed $PASSED, failed $FAILED"
[ "$FAILED" = 0 ] && echo "ALL OK" || exit 1
