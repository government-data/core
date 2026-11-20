#!/bin/bash
# Verification V4 of decision 0073: the Infinity data source in Grafana
# 13.2.5 reading the API's chart routes with the person's own token (OAuth
# passthrough), in an organisation of its own (Quality, id 2) with no other
# data source. The stack is grafana_quality_lib.sh's: one host behind Traefik
# with the repository's middlewares, the provider and the API as stand-ins,
# no network out of it.
#
# It shows: the plugin's archive, licence and signature, loaded with no
# network and no unsigned-plugin setting; what reaches the API when a person
# queries (header names, whose token, its check); typed columns and a null
# kept; and that a Viewer of Quality, calling Grafana's own endpoints
# (/api/ds/query, the data source proxy, its resources, its settings), cannot
# reach another host, a path outside /api/quality/, or another person's
# identity. Only header names and the stand-in's verdicts are printed.
#
# Containers qd-verify-infinity-…, network of that name, removed on exit; no
# port published. Run it alone.
#
#   bash tools/verify/grafana_infinity.sh
P=qd-verify-infinity
NET_OPTS=--internal
. "$(dirname "$0")/machines_lib.sh"
. "$(dirname "$0")/grafana_quality_lib.sh"
start_run
infinity_plugin
quality_files
BASE="https://$APP/api/quality/"
# What the record could add (not in it): Grafana reaching the API by a name
# of its own whose one Traefik router admits GET under /api/quality/datasets/
# alone, after Traefik has cleaned the path, and drops forwarded headers.
API_HOST2=qd-api.verify.internal
cert api2 ca "/O=qd/CN=$API_HOST2" "$API_HOST2"
cp "$D/api2.crt" "$D/api2.key" "$D/c/" && chmod 644 "$D/c/api2".*
cat >"$D/dyn/grafana-api.yml" <<YML
tls:
  certificates: [{ certFile: /c/api2.crt, keyFile: /c/api2.key }]
http:
  routers:
    grafana-api:
      rule: "Host(\`$API_HOST2\`) && Method(\`GET\`) && PathPrefix(\`/api/quality/datasets/\`)"
      entryPoints: [websecure]
      tls: {}
      service: api
      middlewares: [api-chain, grafana-api-headers]
  middlewares:
    grafana-api-headers:
      headers:
        customRequestHeaders: { X-Forwarded-Host: "", X-Forwarded-Tls-Client-Cert: "", X-Forwarded-Tls-Client-Cert-Info: "" }
YML
chmod 644 "$D/dyn/grafana-api.yml"
# Held aside: a provisioned data source of an organisation that does not
# exist yet stops Grafana at boot ([org.notFound], V3).
cat >"$D/quality.yml" <<YML
apiVersion: 1
datasources:
  - name: Quality API
    uid: quality-api
    orgId: 2
    type: yesoreyeram-infinity-datasource
    url: $BASE
    jsonData:
      auth_method: oauthPassThru
      oauthPassThru: true
      allowedHosts: ["$BASE"]
      tlsAuthWithCACert: true
    secureJsonData:
      tlsCACert: \$__file{/c/ca.crt}
  - name: Quality API with no base URL
    uid: quality-nobase
    orgId: 2
    type: yesoreyeram-infinity-datasource
    jsonData:
      auth_method: oauthPassThru
      oauthPassThru: true
      allowedHosts: ["$BASE"]
      tlsAuthWithCACert: true
    secureJsonData:
      tlsCACert: \$__file{/c/ca.crt}
  - name: Quality API through its own router
    uid: quality-routed
    orgId: 2
    type: yesoreyeram-infinity-datasource
    jsonData:
      auth_method: oauthPassThru
      oauthPassThru: true
      allowedHosts: ["https://$API_HOST2/api/quality/datasets/"]
      unsecuredQueryHandling: deny
      tlsAuthWithCACert: true
    secureJsonData:
      tlsCACert: \$__file{/c/ca.crt}
  - name: Quality API with no allowed hosts
    uid: quality-open
    orgId: 2
    type: yesoreyeram-infinity-datasource
    jsonData:
      auth_method: oauthPassThru
      oauthPassThru: true
      tlsAuthWithCACert: true
    secureJsonData:
      tlsCACert: \$__file{/c/ca.crt}
YML
chmod 644 "$D/quality.yml"
quality_edge_up
quality_grafana_up grafana || exit 1

echo "--- the plugin, offline"
expect "Grafana reaches grafana.com" no "$(yn docker exec "$P-grafana" wget -q -T 5 -O /dev/null https://grafana.com/)"
expect "the archive's licence" "Apache License" "$(head -1 "$D/plugins/yesoreyeram-infinity-datasource/LICENSE" | xargs)"
S=$(gfa grafana /api/plugins/yesoreyeram-infinity-datasource/settings)
expect "loaded version, signature, type and signer" "4.1.6 valid grafana Grafana Labs" \
  "$(echo "$S" | python3 -I -c 'import json,sys; d=json.load(sys.stdin); print(d["info"]["version"], d["signature"], d["signatureType"], d["signatureOrg"])')"
expect "allow_loading_unsigned_plugins set" no "$(yn docker exec "$P-grafana" sh -c 'env | grep -qi unsigned')"
note "Grafana's log lines about plugins: $(docker logs "$P-grafana" 2>&1 | grep -ci 'plugin.*\(error\|fail\)')"
# Organisations cannot be provisioned in the open-source build (V3): made
# here by the API, then the data sources of id 2 read again.
expect "organisation Quality made" 2 "$(gfa grafana /api/orgs -X POST -H 'Content-Type: application/json' -d '{"name":"Quality"}' |
  python3 -I -c 'import json,sys; print(json.load(sys.stdin).get("orgId"))')"
install -m 644 "$D/quality.yml" "$D/prov/datasources/"
gfa grafana /api/admin/provisioning/datasources/reload -X POST -o /dev/null
expect "data sources of Quality" "quality-api quality-nobase quality-open quality-routed" \
  "$(gfa grafana /api/datasources -H 'X-Grafana-Org-Id: 2' | python3 -I -c 'import json,sys; print(*sorted(d["uid"] for d in json.load(sys.stdin)))')"

echo "--- what reaches the API"
for who in ana carla; do expect "$who signs in through Quality Data" "200 https://$APP/grafana/" "$(signin $who)"; done
# q URL [EXTRA FIELDS] [DATASOURCE]: one Infinity query, typed columns.
q() {
  printf '[{"refId":"A","datasource":{"type":"yesoreyeram-infinity-datasource","uid":"%s"},"type":"json","source":"url","format":"table","parser":"backend","url":"%s","url_options":{"method":"GET"%s},"columns":[{"selector":"cut","text":"cut","type":"string"},{"selector":"records","text":"records","type":"number"},{"selector":"igcd","text":"igcd","type":"number"}]}]' \
    "${3:-quality-api}" "$1" "${2:-}"
}
# verdict: the response's HTTP status and its frame, or its error's start.
verdict() {
  python3 -I -c 'import json,sys
raw = sys.stdin.read(); code = raw[-3:]
try: r = json.loads(raw[:-3])["results"]["A"]
except Exception: print(code, "no-result"); sys.exit()
if r.get("error"): print(code, "error:", r["error"][:90].replace("\n", " "))
else:
    f = r["frames"][0]; fs = f["schema"]["fields"]; v = f["data"]["values"]
    names = [x["name"] for x in fs]
    ig = "igcd=" + json.dumps(v[names.index("igcd")]) if "igcd" in names else ""
    print(code, "frame", len(v[0]) if v else 0, " ".join(x["name"] + ":" + x["type"] for x in fs), ig)'
}
run() { ds_query "$1" 2 "$2" -w '%{http_code}' | verdict; }
count() { if [ -f "$1" ]; then wc -l <"$1"; else echo 0; fi; }
lines() { echo "$(count "$D/log/api.jsonl") $(count "$D/log/app.jsonl") $(count "$D/log/access.json")"; }
last() { tail -n +"$((${1%% *} + 1))" "$D/log/api.jsonl" 2>/dev/null; }
seen() { last "$1" | python3 -I -c 'import json,sys
for l in sys.stdin:
    e = json.loads(l); print(e["method"], e["host"], e["path"], e["auth"], "id_token:" + e["id_token"])'; }
n=$(lines)
V=$(run ana "$(q "${BASE}datasets/loans/charts/trend_by_cut?cut=2026-10-31")")
expect "ana's chart of loans: typed fields, by name in alphabetical order, a null kept" "200 frame 2 cut:string igcd:number records:number igcd=[0.8, null]" "$V"
expect "what the API received" "GET qd.verify.example /quality/datasets/loans/charts/trend_by_cut ok:ana id_token:ok:ana" "$(seen "$n")"
note "header names at the API: $(last "$n" | python3 -I -c 'import json,sys; print(*json.loads(sys.stdin.readline())["headers"])')"
n=$(lines)
expect "carla's query carries carla's token" "GET qd.verify.example /quality/datasets/savings/charts/trend_by_cut ok:carla id_token:ok:carla" \
  "$(run carla "$(q "${BASE}datasets/savings/charts/trend_by_cut")" >/dev/null; seen "$n")"
n=$(lines)
note "ana asks for savings: $(run ana "$(q "${BASE}datasets/savings/charts/trend_by_cut")")"
expect "the API refused it" "GET qd.verify.example /quality/datasets/savings/charts/trend_by_cut ok:ana id_token:ok:ana" "$(seen "$n")"
expect "and logged it as denied" savings "$(last "$n" | python3 -I -c 'import json,sys; print(json.loads(sys.stdin.readline()).get("denied"))')"
n=$(lines)
note "a relative URL against the base: $(run ana "$(q "datasets/loans/charts/trend_by_cut")") → $(seen "$n" | tr '\n' ' ')"

echo "--- a Viewer of Quality aiming elsewhere"
# Where a request landed: refused (nothing arrived), chart (a chart route),
# prefix (under /api/quality/, not a chart route), ESCAPED (another path of
# the host) or OTHERHOST; with whose token.
where() { python3 -I - "$D/log" $1 <<'PY'
import json, sys
d, na, np, nt = sys.argv[1], *map(int, sys.argv[2:5])
def new(f, n):
    try: return [json.loads(l) for l in open(f"{d}/{f}").readlines()[n:]]
    except FileNotFoundError: return []
out = []
for e in new("api.jsonl", na) + new("app.jsonl", np):
    p = e["path"]
    c = ("OTHERHOST" if not e["host"].startswith(("qd.verify.example", "qd-api.verify.internal"))
         else "chart" if e["role"] == "api" and p.startswith("/quality/datasets")
         else "prefix" if e["role"] == "api" and p.startswith("/quality/") else "ESCAPED")
    out.append(" ".join([c, e["role"], e["method"], p, e.get("auth", "-")]))
if not out:
    t = [f'{e.get("DownstreamStatus")}' for e in new("access.json", nt) if e.get("RouterName") != "grafana@file"]
    out.append("refused" + (" (Traefik " + ",".join(t) + ")" if t else ""))
print("; ".join(out))
PY
}
# try_url DATASOURCE URL EXPECTED [EXTRA FIELDS]: one query as ana.
try_url() {
  local n r w
  n=$(lines)
  r=$(run ana "$(q "$2" "${4:-}" "$1")")
  w=$(where "$n")
  echo "       $1 $2${4:+ $4} → $r"
  expect "$1 $2${4:+ $4}" "$3" "$w"
}
# The table: data source | URL | extra query fields | where it must land.
# quality-api has the base URL (an absolute URL is appended to it),
# quality-nobase none, quality-routed goes through the router above.
E=evil.verify.example R=https://$API_HOST2/api/quality/datasets/ PO=',"method":"POST","data":"{}"'
while IFS='|' read -r ds url extra want; do
  [ -z "$ds" ] && continue
  try_url "$ds" "$url" "$want" "$extra"
done <<TABLE
quality-api|https://$E/quality/datasets/loans||prefix api GET /quality/https:/$E/quality/datasets/loans ok:ana
quality-api|http://$E:8080/quality/datasets/loans||prefix api GET /quality/http:/$E:8080/quality/datasets/loans ok:ana
quality-api|https://$APP/api/internal/jobs/claim||prefix api GET /quality/https:/$APP/api/internal/jobs/claim ok:ana
quality-api|https://$APP/api/quality@$E:8080/quality/datasets/loans||prefix api GET /quality/https:/$APP/api/quality@$E:8080/quality/datasets/loans ok:ana
quality-api|../internal/jobs/claim||ESCAPED api GET /internal/jobs/claim ok:ana
quality-nobase|https://$E/quality/datasets/loans||refused
quality-nobase|http://$E:8080/quality/datasets/loans||refused
quality-nobase|https://$APP/api/internal/jobs/claim||refused
quality-nobase|https://$APP/api/quality@$E:8080/quality/datasets/loans||refused
quality-nobase|https://$APP/api/qualityx/datasets||refused
quality-open|http://$E:8080/quality/datasets/loans||refused
quality-nobase|${BASE}../internal/jobs/claim||ESCAPED api GET /internal/jobs/claim ok:ana
quality-nobase|${BASE}%2e%2e/internal/jobs/claim||ESCAPED api GET /internal/jobs/claim ok:ana
quality-nobase|${BASE}datasets/../../internal/jobs/claim||ESCAPED api GET /internal/jobs/claim ok:ana
quality-nobase|${BASE}..%2finternal/jobs/claim||prefix api GET /quality/..%2Finternal/jobs/claim ok:ana
quality-nobase|${BASE}../oauth/userinfo||ESCAPED app GET /oauth/userinfo ok:ana
quality-nobase|${BASE}datasets/loans/charts/trend_by_cut|$PO|chart api POST /quality/datasets/loans/charts/trend_by_cut ok:ana
quality-nobase|${BASE}../internal/jobs/claim|$PO|refused (Traefik 501)
quality-nobase|${BASE}datasets/loans/charts/trend_by_cut|,"headers":[{"key":"Authorization","value":"Bearer forged"}]|chart api GET /quality/datasets/loans/charts/trend_by_cut bad
quality-routed|${R}loans/charts/trend_by_cut||chart api GET /quality/datasets/loans/charts/trend_by_cut ok:ana
quality-routed|${R}../../internal/jobs/claim||refused (Traefik 404)
quality-routed|${R}%2e%2e/%2e%2e/internal/jobs/claim||refused (Traefik 404)
quality-routed|${R}../../oauth/userinfo||refused (Traefik 404)
quality-routed|${R}loans/charts/trend_by_cut|$PO|refused (Traefik 404)
quality-routed|${R}loans/charts/trend_by_cut|,"headers":[{"key":"Authorization","value":"Bearer forged"}]|refused
quality-nobase|${BASE}datasets/loans/charts/trend_by_cut|,"headers":[{"key":"X-Forwarded-Tls-Client-Cert","value":"forged"},{"key":"X-Qd-Probe","value":"1"}]|chart api GET /quality/datasets/loans/charts/trend_by_cut ok:ana
quality-routed|${R}loans/charts/trend_by_cut|,"headers":[{"key":"X-Forwarded-Tls-Client-Cert","value":"forged"},{"key":"X-Qd-Probe","value":"1"}]|chart api GET /quality/datasets/loans/charts/trend_by_cut ok:ana
TABLE
hdr() { tail -n "$1" "$D/log/api.jsonl" | head -1 | python3 -I -c 'import json,sys; h = json.loads(sys.stdin.readline())["headers"]; print("x-qd-probe" in h, "x-forwarded-tls-client-cert" in h)'; }
expect "a query's own headers at the API, public router: X-Qd-Probe, X-Forwarded-Tls-Client-Cert (Traefik drops it)" "True False" "$(hdr 2)"
expect "and through the router of its own" "True False" "$(hdr 1)"

echo "--- the data source proxy and resources, as ana"
while IFS='|' read -r t want; do
  [ -z "$t" ] && continue
  n=$(lines)
  c=$(as ana "${t#* }" -X "${t%% *}" --path-as-is -H 'X-Grafana-Org-Id: 2' -o /dev/null -w '%{http_code}')
  echo "       $t → $c"
  expect "$t" "$want" "$(where "$n")"
done <<TABLE
GET /api/datasources/proxy/uid/quality-api/datasets/loans/charts|chart api GET /quality/datasets/loans/charts ok:ana
POST /api/datasources/proxy/uid/quality-api/datasets/loans/charts|chart api POST /quality/datasets/loans/charts ok:ana
GET /api/datasources/proxy/uid/quality-api/../../internal/jobs/claim|refused
GET /api/datasources/proxy/uid/quality-api/%2e%2e/internal/jobs/claim|refused
GET /api/datasources/proxy/uid/quality-api/..%2finternal/jobs/claim|ESCAPED api GET /internal/jobs/claim ok:ana
GET /api/datasources/proxy/uid/quality-api/datasets%2f..%2f..%2finternal/jobs/claim|ESCAPED api GET /internal/jobs/claim ok:ana
GET /api/datasources/proxy/uid/quality-nobase/datasets/loans/charts|refused
GET /api/datasources/proxy/uid/quality-routed/loans/charts|refused
GET /api/datasources/uid/quality-api/resources/datasets/loans/charts|refused
GET /api/plugins/yesoreyeram-infinity-datasource/resources/api/v1/settings/terms|refused
TABLE
expect "ana changes the data source's allowed hosts" 403 "$(as ana /api/datasources/uid/quality-api -X PUT -H 'X-Grafana-Org-Id: 2' \
  -H 'Content-Type: application/json' -d '{"name":"Quality API","type":"yesoreyeram-infinity-datasource","access":"proxy","jsonData":{"allowedHosts":["http://evil.verify.example:8080/"]}}' -o /dev/null -w '%{http_code}')"
expect "ana adds a data source" 403 "$(as ana /api/datasources -X POST -H 'X-Grafana-Org-Id: 2' -H 'Content-Type: application/json' \
  -d '{"name":"x","type":"yesoreyeram-infinity-datasource","access":"proxy"}' -o /dev/null -w '%{http_code}')"
expect "requests at host evil, all told" 0 "$(api_log 'e["host"].startswith("evil")' | wc -l)"
note "requests outside /quality/datasets, all told: $(api_log 'e.get("other")' | wc -l) at the API, $(api_log 'e["path"] != "/oauth/token" and e["path"] != "/oauth/authorize" and e["path"] != "/test/signin"' app | wc -l) at the provider"

echo
echo "passed $PASSED, failed $FAILED"
[ "$FAILED" = 0 ] && echo "ALL OK" || exit 1
