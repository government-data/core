#!/bin/bash
# Verification V3 of decision 0073: a second Grafana organisation, Quality,
# in the open-source 13.2.5, people mapped to it from a list claim
# (grafana_orgs) by generic OAuth's org_attribute_path and org_mapping, and
# that its members cannot reach the data sources of the first (the
# repository's PostgreSQL, Loki and Mimir files, provisioned unchanged). The
# stack is grafana_quality_lib.sh's; the first organisation's data sources
# point at the API stand-in, so a query that reached them would be logged.
#
# It shows: what provisioning does with an organisation that does not exist;
# how the organisation is made and its id; the mapping with and without a
# role under role_attribute_strict; who lands where (ana Quality, bruno the
# audit, carla both, root admin of both, dario nobody); a claim withdrawn at
# the next sign-in; how a person of both picks the organisation of a request;
# and each way a member of Quality tries the other organisation's data
# sources, refused. Only names, roles and verdicts are printed.
#
# Containers qd-verify-orgs-…, network of that name, removed on exit; no
# port published. Run it alone.
#
#   bash tools/verify/grafana_orgs.sh
P=qd-verify-orgs
NET_OPTS=--internal
. "$(dirname "$0")/machines_lib.sh"
. "$(dirname "$0")/grafana_quality_lib.sh"
start_run
infinity_plugin
quality_files
SRC=$ROOT/infra/observability/grafana/provisioning/datasources
for f in postgres loki mimir; do install -m 644 "$SRC/$f.yml" "$D/prov/datasources/"; done
printf 'throwaway' >"$D/c/pg_password" && chmod 644 "$D/c/pg_password"
FIRST=(-e GRAFANA_PG_HOST=api -e GRAFANA_PG_PORT=8080 -e GRAFANA_PG_DATABASE=knime -e GRAFANA_PG_SSLMODE=disable
  -e LOKI_URL=http://api:8080 -e MIMIR_URL=http://api:8080 -e MIMIR_SERVER_NAME= -v "$D/c/pg_password:/run/secrets/grafana_pg_password:ro")
cat >"$D/quality-ds.yml" <<YML
apiVersion: 1
datasources:
  - name: Quality API
    uid: quality-api
    orgId: 2
    type: yesoreyeram-infinity-datasource
    jsonData:
      auth_method: oauthPassThru
      oauthPassThru: true
      allowedHosts: ["https://$APP/api/quality/"]
      tlsAuthWithCACert: true
    secureJsonData:
      tlsCACert: \$__file{/c/ca.crt}
YML
cat >"$D/quality-dash.yml" <<'YML'
apiVersion: 1
providers:
  - { name: quality, orgId: 2, type: file, disableDeletion: true, allowUiUpdates: false, options: { path: /dash/quality } }
  - { name: audit, orgId: 1, type: file, disableDeletion: true, allowUiUpdates: false, options: { path: /dash/audit } }
YML
mkdir -p "$D/dash/quality" "$D/dash/audit"
printf '{"uid":"qd-quality-en","title":"Quality indicators","schemaVersion":42,"panels":[]}' >"$D/dash/quality/q.json"
printf '{"uid":"qd-audit-en","title":"Audit trail","schemaVersion":42,"panels":[]}' >"$D/dash/audit/a.json"
chmod -R a+rX "$D/dash" && chmod 644 "$D"/quality-*.yml
quality_edge_up

echo "--- provisioning an organisation that does not exist"
running() { docker ps -q --filter "name=^/$P-grafana\$" | grep -q . && echo yes || echo no; }
said() { docker logs "$P-grafana" 2>&1 | grep -q "$1" && echo yes || echo no; }
install -m 644 "$D/quality-ds.yml" "$D/prov/datasources/"
quality_grafana_up grafana "${FIRST[@]}" >/dev/null 2>&1
expect "Grafana with a data source of organisation 2, which does not exist, keeps running" no "$(running)"
expect "and says why" yes "$(said 'Datasource provisioning error.*org.notFound.*failed to get org by ID: 2')"
docker rm -f "$P-grafana" >/dev/null
rm "$D/prov/datasources/quality-ds.yml"
install -m 644 "$D/quality-dash.yml" "$D/prov/dashboards/"
quality_grafana_up grafana "${FIRST[@]}" >/dev/null 2>&1
expect "Grafana with a dashboards provider of organisation 2 keeps running" no "$(running)"
expect "and says why" yes "$(said 'failed to provision dashboards with "quality" reader: \[org.notFound\]')"
docker rm -f "$P-grafana" >/dev/null
rm "$D/prov/dashboards/quality-dash.yml"
quality_grafana_up grafana "${FIRST[@]}" || exit 1
ok "Grafana starts with organisation 1's files alone"
expect "organisations a fresh Grafana has" "1 Main Org." "$(gfa grafana /api/orgs | python3 -I -c 'import json,sys; print(*[f"{o["id"]} {o["name"]}" for o in json.load(sys.stdin)], sep="; ")')"
expect "Quality made by the API, its id" 2 "$(gfa grafana /api/orgs -X POST -H 'Content-Type: application/json' -d '{"name":"Quality"}' |
  python3 -I -c 'import json,sys; print(json.load(sys.stdin).get("orgId"))')"
install -m 644 "$D/quality-ds.yml" "$D/prov/datasources/"
install -m 644 "$D/quality-dash.yml" "$D/prov/dashboards/"
gfa grafana /api/admin/provisioning/datasources/reload -X POST -o /dev/null
gfa grafana /api/admin/provisioning/dashboards/reload -X POST -o /dev/null
docker restart "$P-grafana" >/dev/null
for i in $(seq 60); do [ "$(gfa grafana /api/health -o /dev/null -w '%{http_code}')" = 200 ] && break; sleep 2; done
dsof() { gfa grafana /api/datasources -H "X-Grafana-Org-Id: $1" | python3 -I -c 'import json,sys; print(*sorted(d["uid"] for d in json.load(sys.stdin)))'; }
expect "after a restart, data sources of organisation 1" "loki mimir postgres" "$(dsof 1)"
expect "and of Quality" "quality-api" "$(dsof 2)"
dashof() { gfa grafana "/api/search?type=dash-db" -H "X-Grafana-Org-Id: $1" | python3 -I -c 'import json,sys; print(*sorted(d["uid"] for d in json.load(sys.stdin)))'; }
expect "dashboards of organisation 1, provisioned from files" "qd-audit-en" "$(dashof 1)"
expect "and of Quality" "qd-quality-en" "$(dashof 2)"

echo "--- who lands where"
orgs() { as "$1" /api/user/orgs | python3 -I -c 'import json,sys
try: print(*[f"{o["orgId"]}:{o["role"]}" for o in json.load(sys.stdin)] or ["none"])
except Exception: print("refused")'; }
for t in "ana 2:Viewer" "bruno 1:Viewer" "carla 1:Viewer 2:Viewer" "root 1:Admin 2:Admin" "dario refused"; do
  who=${t%% *}
  signin "$who" >/dev/null
  expect "$who's organisations and roles" "${t#* }" "$(orgs "$who")"
done
expect "the mapping skipped for lack of a role, logged" 0 "$(docker logs "$P-grafana" 2>&1 | grep -c 'Skipping org mapping' | cut -c1)"
expect "ana's current organisation" 2 "$(as ana /api/user | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["orgId"])')"

echo "--- a person of both organisations"
cur() { as carla /api/user "$@" | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["orgId"])'; }
note "carla's current organisation after sign-in: $(cur)"
expect "carla's search with X-Grafana-Org-Id 2" qd-quality-en "$(as carla '/api/search?type=dash-db' -H 'X-Grafana-Org-Id: 2' | python3 -I -c 'import json,sys; print(*[d["uid"] for d in json.load(sys.stdin)])')"
expect "and with 1" qd-audit-en "$(as carla '/api/search?type=dash-db' -H 'X-Grafana-Org-Id: 1' | python3 -I -c 'import json,sys; print(*[d["uid"] for d in json.load(sys.stdin)])')"
boot() { as carla "$1" -L | python3 -I -c 'import re,sys; m = re.search(r"\"orgCount\":\d+,\"orgId\":(\d+)", sys.stdin.read()); print(m.group(1) if m else "none")'; }
expect "the page with ?orgId=2: the organisation in its boot data" 2 "$(boot '/d/qd-quality-en/?orgId=2')"
expect "with ?orgId=1" 1 "$(boot '/d/qd-audit-en/?orgId=1')"
expect "with ?orgId=2 again" 2 "$(boot '/d/qd-quality-en/?orgId=2')"
note "carla's stored organisation after those pages: $(cur)"
expect "Grafana's frontend sends X-Grafana-Org-Id on its API calls (13.2.5 bundle)" yes \
  "$(yn docker exec "$P-grafana" sh -c 'grep -lq "X-Grafana-Org-Id" public/build/*.js')"

echo "--- ana, of Quality alone, against organisation 1's data sources"
api_n() { if [ -f "$D/log/api.jsonl" ]; then wc -l <"$D/log/api.jsonl"; else echo 0; fi; }
n0=$(api_n)
pg='{"refId":"A","datasource":{"type":"grafana-postgresql-datasource","uid":"postgres"},"rawSql":"SELECT 1","format":"table"}'
lk='{"refId":"A","datasource":{"type":"loki","uid":"loki"},"expr":"{vm=~\".+\"}","queryType":"range"}'
mx='{"refId":"A","datasource":{"type":"datasource","uid":"-- Mixed --"}},{"refId":"B","datasource":{"type":"prometheus","uid":"mimir"},"expr":"up"}'
ex='{"refId":"A","datasource":{"type":"prometheus","uid":"mimir"},"expr":"up"},{"refId":"B","datasource":{"type":"__expr__","uid":"__expr__"},"type":"math","expression":"$A"}'
for t in "postgres in org 2|2|[$pg]" "postgres in org 1|1|[$pg]" "loki in org 2|2|[$lk]" "mixed with mimir|2|[$mx]" "an expression over mimir|2|[$ex]"; do
  IFS='|' read -r name org body <<<"$t"
  r=$(ds_query ana "$org" "$body" -w '\n%{http_code}' | python3 -I -c 'import json,sys
body, code = sys.stdin.read().rsplit("\n", 1)
try: d = json.loads(body)
except Exception: d = {}
errs = [r.get("error", "") for r in d.get("results", {}).values()] or [d.get("message", body[:80])]
print(code, "|".join(e[:70] for e in errs))')
  if [ "$r" = "200 " ]; then ko "ana's $name answered with data"; else ok "ana's $name refused: $r"; fi
done
for t in "POST /api/user/using/1" "GET /api/datasources/uid/postgres" "GET /api/datasources/uid/postgres/health" \
  "GET /api/datasources/proxy/uid/loki/loki/api/v1/labels" "GET /api/datasources/uid/mimir/resources/api/v1/labels" \
  "GET /api/datasources/proxy/uid/mimir/api/v1/query?query=up" "GET /api/datasources/1" "GET /d/qd-audit-en/?orgId=1"; do
  c=$(as ana "${t#* }" -X "${t%% *}" -H 'X-Grafana-Org-Id: 1' -o /dev/null -w '%{http_code}')
  case "$c" in 2*) ko "ana: $t answered $c" ;; *) ok "ana: $t refused ($c)" ;; esac
done
expect "ana's current organisation, after all that" 2 "$(as ana /api/user | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["orgId"])')"
expect "requests that reached organisation 1's data sources (the stand-in)" 0 $(($(api_n) - n0))
gfa grafana /api/ds/query -X POST -H 'Content-Type: application/json' -H 'X-Grafana-Org-Id: 1' \
  -d "{\"from\":\"now-1h\",\"to\":\"now\",\"queries\":[{\"refId\":\"A\",\"datasource\":{\"type\":\"prometheus\",\"uid\":\"mimir\"},\"expr\":\"up\"}]}" -o /dev/null
expect "the same Mimir query by the admin in organisation 1 does reach it (control)" yes \
  "$([ "$(api_n)" -gt "$n0" ] && echo yes || echo no)"

echo "--- a claim withdrawn"
python3 -I -c 'import json,sys; p=json.load(open(sys.argv[1])); p["ana"]["grafana_orgs"]=[]; p["ana"]["policy"]=[]; json.dump(p, open(sys.argv[1], "w"))' "$D/c/people.json"
expect "ana's live session still works until it ends" 200 "$(as ana /api/user -o /dev/null -w '%{http_code}')"
rm -f "$D/jar/ana"
expect "a new sign-in of ana is refused" "200 https://$APP/grafana/login" "$(signin ana)"

echo
echo "passed $PASSED, failed $FAILED"
[ "$FAILED" = 0 ] && echo "ALL OK" || exit 1
