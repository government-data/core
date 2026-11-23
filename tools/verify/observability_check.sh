#!/bin/sh
# Verification for decision 0026 against the dev stacks: the application's
# Traefik, the lake (task lake:up) and observability (task obs:up). From the
# repository root:
#   tools/verify/observability_check.sh
# Grafana through Traefik and its sign-in with Quality Data; Loki ready and
# receiving the containers' logs from Alloy; Loki's objects in the lake;
# Grafana's dashboard provider and the request id's link in its datasource;
# the generated dashboards, provisioned, and every query of theirs;
# Grafana's secrets read from files; Grafana's PostgreSQL role and datasource;
# the level label on each service's lines; OBS_VM in the compose files'
# environment; the API's line of a request in Loki by its request id (in
# watch mode through .dev/logs, alloy/dev-host.alloy); a host process's text
# line labelled; the latest run's lines by its run_id. Prints no secret;
# RustFS is read with Loki's own keys from
# .env, Grafana with its emergency admin, whose password reaches curl on its
# standard input, never on a command line.
set -u
cd "$(dirname "$0")/../.."
set -a; . ./.env; set +a
domain=${APP_DOMAIN:-localhost}
ns=${APP_STACK:-data-quality}-traefik-1
bucket=${LOKI_S3_BUCKET:-qd-observability}
secrets=${QD_SECRETS_DIR:-./secrets}
fails=0
ok() { echo "[OK  ] $1"; }
ko() { echo "[FAIL] $1"; fails=$((fails + 1)); }
# HTTP inside Traefik's network namespace, where Loki listens (not exposed).
inside() {
  docker run --rm --network "container:$ns" curlimages/curl:8.11.1 -s "$@"
}
# A secret file's value, quoted for a curl config line.
quoted() { sed 's/[\\"]/\\&/g' "$secrets/$1"; }
# The same, with curl's configuration (the secrets) on its standard input.
inside_with() {
  docker run --rm -i --network "container:$ns" curlimages/curl:8.11.1 -s -K - "$@"
}
# As Grafana's emergency admin, its password read from its secret file.
as_admin() {
  printf 'user = "%s:%s"\n' "${GRAFANA_ADMIN_USER:-qd-emergency-admin}" "$(quoted grafana_admin_password)" |
    inside_with "$@"
}

# 1. Grafana, through Traefik.
health=$(curl -sk "https://$domain/grafana/api/health")
case "$health" in
  *'"database"'*'"ok"'*) ok "grafana health via traefik: $(echo "$health" | tr -d ' \n')" ;;
  *) ko "grafana health via traefik: $health" ;;
esac

# 2. Framing: only this domain (app-frame-headers), no DENY from Grafana.
headers=$(curl -sk -D - -o /dev/null "https://$domain/grafana/login")
if echo "$headers" | grep -qi "frame-ancestors 'self'" && ! echo "$headers" | grep -qi 'x-frame-options: deny'; then
  ok "grafana may be framed by $domain only"
else
  ko "grafana frame headers: $(echo "$headers" | grep -i 'frame' | tr -d '\r' | tr '\n' ' ')"
fi

# 3. The sign-in goes to Quality Data's authorize endpoint, with PKCE.
location=$(curl -sk -o /dev/null -w '%{redirect_url}' "https://$domain/grafana/login/generic_oauth")
case "$location" in
  "https://$domain/api/oauth/authorize?"*client_id=grafana*code_challenge=*)
    ok "oauth redirect: https://$domain/api/oauth/authorize?client_id=grafana…" ;;
  *) ko "oauth redirect: ${location:-none}" ;;
esac
# And with auto-login, the start page goes straight to it.
start=$(curl -sk -o /dev/null -w '%{redirect_url}' "https://$domain/grafana/login")
case "$start" in
  *"/grafana/login/generic_oauth"*) ok "auto-login: /grafana/login → generic_oauth" ;;
  *) ko "auto-login: /grafana/login → ${start:-no redirect}" ;;
esac

# 4. Loki ready (it waits 15 s after start), and not routed by Traefik:
# from outside, /loki is the web's page, not Loki's JSON.
for _ in 1 2 3 4 5 6; do
  ready=$(inside "http://127.0.0.1:${LOKI_HTTP_PORT:-3100}/ready")
  [ "$ready" = ready ] && break
  sleep 5
done
[ "$ready" = ready ] && ok "loki ready" || ko "loki ready: $ready"
case "$(curl -sk "https://$domain/loki/api/v1/labels")" in
  *'"status":"success"'*) ko "loki is reachable through traefik" ;;
  *) ok "loki not exposed through traefik" ;;
esac

# 5. Alloy ships: the lake's container logs, with their labels.
labels=$(inside "http://127.0.0.1:${LOKI_HTTP_PORT:-3100}/loki/api/v1/labels")
for label in container service compose_project vm; do
  case "$labels" in *"\"$label\""*) ok "label $label" ;; *) ko "label $label missing: $labels" ;; esac
done
projects=$(inside "http://127.0.0.1:${LOKI_HTTP_PORT:-3100}/loki/api/v1/label/compose_project/values")
echo "       compose_project values: $projects"
lines=$(inside -G "http://127.0.0.1:${LOKI_HTTP_PORT:-3100}/loki/api/v1/query_range" \
  --data-urlencode 'query={compose_project="data-quality-lake"}' --data-urlencode 'limit=5' --data-urlencode 'since=24h' |
  grep -o '"values":\[\[' | wc -l)
[ "$lines" -gt 0 ] && ok "{compose_project=\"data-quality-lake\"}: $lines stream(s)" || ko "no lines for {compose_project=\"data-quality-lake\"}"
case "$projects" in *calidad-datos*) ko "calidad-datos is collected" ;; esac

# 6. Loki's objects in the lake, read with its own keys (index and chunks
# appear once Loki flushes; `/flush` forces it).
inside -X POST -o /dev/null "http://127.0.0.1:${LOKI_HTTP_PORT:-3100}/flush"
sleep "${FLUSH_WAIT:-15}"
listing=$(inside --aws-sigv4 "aws:amz:us-east-1:s3" --user "$LOKI_S3_ACCESS_KEY:$LOKI_S3_SECRET_KEY" \
  "http://127.0.0.1:9000/$bucket?list-type=2&max-keys=1000")
objects=$(echo "$listing" | grep -o '<Key>' | wc -l)
[ "$objects" -gt 0 ] && ok "objects in $bucket: $objects (first page)" || ko "no objects in $bucket yet: $(echo "$listing" | head -c 200)"
# Loki's user sees only its bucket, and cannot make another.
seen=$(inside --aws-sigv4 "aws:amz:us-east-1:s3" --user "$LOKI_S3_ACCESS_KEY:$LOKI_S3_SECRET_KEY" \
  "http://127.0.0.1:9000/" | grep -o '<Name>[^<]*</Name>' | sed 's/<[^>]*>//g' | tr '\n' ' ')
[ "$seen" = "$bucket " ] && ok "loki's user sees only $bucket" || ko "loki's user sees: $seen"
made=$(inside -o /dev/null -w '%{http_code}' -X PUT --aws-sigv4 "aws:amz:us-east-1:s3" \
  --user "$LOKI_S3_ACCESS_KEY:$LOKI_S3_SECRET_KEY" "http://127.0.0.1:9000/qd-observability-probe")
[ "$made" = 403 ] && ok "loki's user denied a new bucket" || ko "loki's user, new bucket: HTTP $made"

# 7. Grafana listed in the avatar's «Aplicaciones» (a row with home and detail).
row=$(docker exec "${APP_STACK:-data-quality}-postgres-1" psql -U "${POSTGRES_SUPERUSER:-postgres}" -d "${POSTGRES_DB:-quality_data}" -tA \
  -c "SELECT name || ' | ' || detail || ' | ' || home_url FROM oidc_clients WHERE client_id = 'grafana'")
[ -n "$row" ] && ok "oidc client grafana: $row" || ko "oidc client grafana not registered"

# 8. The dashboard provider (provisioning/dashboards/files.yml): Grafana's
# user reads it and its directory, and a reload logs no provisioning error
# (Grafana answers 200 to a reload even when it cannot read the provider).
grafana=data-quality-observability-grafana-1
gport=${GRAFANA_PORT:-3000}
if docker exec "$grafana" sh -c 'grep -q "path: /var/lib/grafana-dashboards" /etc/grafana/provisioning/dashboards/files.yml && ls /var/lib/grafana-dashboards >/dev/null'; then
  ok "dashboard provider readable by grafana, with its directory"
else
  ko "dashboard provider or /var/lib/grafana-dashboards not readable in $grafana"
fi
since=$(date -u +%Y-%m-%dT%H:%M:%SZ)
reload=$(as_admin -X POST "http://127.0.0.1:$gport/api/admin/provisioning/dashboards/reload")
sleep 2
errors=$(docker logs --since "$since" "$grafana" 2>&1 | grep '"logger":"provisioning.dashboard"' | grep '"level":"error"')
case "$reload" in
  *reloaded*) [ -z "$errors" ] && ok "dashboard provider reloaded, no provisioning error" || ko "dashboard provisioning: $errors" ;;
  *) ko "dashboard provider reload: $reload" ;;
esac

# 9. The Loki datasource links a request, job, run and Trino query id to
# every line carrying it, with `${__value.raw}` intact (`$$` in the
# provisioning file), each field's regex taking the id from a JSON line and
# from a pretty one (`key=value`, colour codes around), as Grafana runs it.
loki_ds=$(as_admin "http://127.0.0.1:$gport/api/datasources/uid/loki")
for field in request_id job_id run_id trino_query_id; do
  case "$loki_ds" in
    *"\"name\":\"$field\""*) ok "loki derived field $field" ;;
    *) ko "loki derived field $field missing" ;;
  esac
done
if echo "$loki_ds" | node -e '
  const fields = JSON.parse(require("fs").readFileSync(0, "utf8")).jsonData.derivedFields;
  const id = { request_id: "00000000-0000-4000-8000-00000000000a.0123456789abcdef",
    job_id: "00000000-0000-4000-8000-00000000000a", run_id: "00000000-0000-4000-8000-00000000000b",
    trino_query_id: "20261112_000000_00001_abcde" };
  const bad = fields.filter((f) => f.datasourceUid !== "loki" ||
    f.url !== "{vm=~\".+\"} |= \"${__value.raw}\"" ||
    [`{"${f.name}":"${id[f.name]}"}`, `\x1b[3m${f.name}\x1b[0m\x1b[2m=\x1b[0m${id[f.name]} x=1`]
      .some((line) => new RegExp(f.matcherRegex).exec(line)?.[1] !== id[f.name]));
  process.exit(fields.length === 4 && bad.length === 0 ? 0 : 1);'; then
  ok "each id opens an Explore query in loki for that id, from JSON and pretty lines"
else
  ko "derived field regex or link: $(echo "$loki_ds" | grep -o '"derivedFields":\[.*\]')"
fi

# 10. Grafana's secrets are files (docker-compose.yml, setup-secrets.sh): the
# folder 700, the files 444 so Grafana's user (472) reads them; its
# configuration names only the files, with neither value; the emergency admin
# signs in with the file's password, and Quality Data accepts the file's
# client secret (a probe code: `invalid_grant`, not `invalid_client`).
[ "$(stat -c %a "$secrets")" = 700 ] && ok "secrets folder 700" || ko "secrets folder: $(stat -c %a "$secrets")"
for name in grafana_admin_password grafana_oidc_client_secret grafana_pg_password; do
  [ "$(stat -c %a "$secrets/$name")" = 444 ] && ok "$name 444" || ko "$name: $(stat -c %a "$secrets/$name")"
  docker exec "$grafana" test -s "/run/secrets/$name" && ok "$name readable by grafana" || ko "$name not readable in $grafana"
done
env=$(docker inspect -f '{{range .Config.Env}}{{println .}}{{end}}' "$grafana")
files=$(echo "$env" | grep -c -e '^GF_SECURITY_ADMIN_PASSWORD__FILE=/run/secrets/grafana_admin_password$' \
  -e '^GF_AUTH_GENERIC_OAUTH_CLIENT_SECRET__FILE=/run/secrets/grafana_oidc_client_secret$')
[ "$files" = 2 ] && ok "grafana reads its secrets through __FILE" || ko "grafana's __FILE variables: $files of 2"
if echo "$env" | grep -q -e '^GF_SECURITY_ADMIN_PASSWORD=' -e '^GF_AUTH_GENERIC_OAUTH_CLIENT_SECRET='; then
  ko "grafana's secrets as plain variables"
elif echo "$env" | grep -qF -f "$secrets/grafana_admin_password" -f "$secrets/grafana_oidc_client_secret" \
  -f "$secrets/grafana_pg_password"; then
  ko "a secret's value in grafana's configuration"
else
  ok "no secret value in grafana's configuration"
fi
org=$(as_admin -o /dev/null -w '%{http_code}' "http://127.0.0.1:$gport/api/org")
[ "$org" = 200 ] && ok "emergency admin signs in with the file's password" || ko "emergency admin, /api/org: HTTP $org"
verifier=$(printf 'probe%.0s' 1 2 3 4 5 6 7 8 9)
token=$(printf 'data = "grant_type=authorization_code&code=probe&client_id=grafana&code_verifier=%s"\n%s\n%s\n' \
  "$verifier" "data-urlencode = \"redirect_uri=https://$domain/grafana/login/generic_oauth\"" \
  "data-urlencode = \"client_secret=$(quoted grafana_oidc_client_secret)\"" | inside_with -k "https://$domain/api/oauth/token")
case "$token" in
  *'"invalid_grant"'*) ok "quality data accepts grafana's client secret" ;;
  *) ko "grafana's client secret: $token" ;;
esac

# 11. Grafana's PostgreSQL role (infra/postgres/grafana-role.sh): as
# qd_grafana, over TCP inside the database's container, its password from
# the file in the environment of `docker exec`, never on a command line.
# Prints counts and errors only, never a row.
pg=${POSTGRES_CONTAINER:-${APP_STACK:-data-quality}-postgres-1}
db=${POSTGRES_DB:-quality_data}
as_grafana_pg() {
  PGPASSWORD=$(cat "$secrets/grafana_pg_password") docker exec -i -e PGPASSWORD "$pg" \
    psql -X -qAt -v ON_ERROR_STOP=1 -h 127.0.0.1 -U qd_grafana -d "$db" 2>&1
}
as_super_pg() { docker exec -i "$pg" psql -X -qAt -v ON_ERROR_STOP=1 -U "${POSTGRES_SUPERUSER:-postgres}" -d "$db" 2>&1; }
granted="public.admin_audit_events id,tenant_id,occurred_at,event_type,admin_id,action,resource,service_account,job_id,ip,user_agent,email_attempted,purpose,reference,rows_shown,fields_shown
public.admin_users id,tenant_id,email,display_name
public.machines id,tenant_id,name,role,pool,state,version,cert_not_after,created_at,approved_at,retired_at
quality.jobs id,tenant_id,kind,dataset,engine,state,priority,attempts,max_attempts,not_before,worker,lease_until,created_at,started_at,finished_at,uses_trino
quality.cuts id,tenant_id,dataset,cut,schedule_id,requested_by,state,run,extract_job,evaluate_job,due_at,fired_at
quality.run_summary tenant_id,dataset,cut,job_id,records,duration_ms,rules_commit,reported_at,igcd,entities,band_high,band_medium,band_low
quality.job_stats tenant_id,dataset,cut,stage,position,ms
quality.rule_results tenant_id,dataset,cut,rule,applies,failures,field,pattern,dimension,severity
quality.trino_holds tenant_id,machine,held_at,expires_at
quality.field_quality tenant_id,dataset,cut,field,empty,distinct_values,pattern,pattern_count,mean_icd,recency
quality.entity_icd tenant_id,dataset,cut,entity,icd,band,segment
quality.group_metrics tenant_id,dataset,cut,dimension,grp,records,serious,mean_icd
quality.distribution_checks tenant_id,dataset,cut,variable,dimension,grp,percentiles,records,low,high
quality.queue_findings tenant_id,dataset,cut,dimension,grp,since,open,resolved,priority
quality.cells tenant_id,dataset,cut,cell,ok,total,passes,missing,margin
quality.remediation_slas tenant_id,dataset,dimension,grp,days"
# Exactly these columns are granted, and nothing else in any schema.
want=$(echo "$granted" | while read -r t cols; do
  echo "$t $(echo "$cols" | tr ',' '\n' | LC_ALL=C sort | paste -sd, -)"
done | LC_ALL=C sort)
have=$(echo "SELECT table_schema || '.' || table_name || ' ' || string_agg(column_name, ',' ORDER BY column_name COLLATE \"C\")
  FROM information_schema.column_privileges WHERE grantee = 'qd_grafana' GROUP BY table_schema, table_name" |
  as_super_pg | LC_ALL=C sort)
[ "$have" = "$want" ] && ok "qd_grafana's grants are exactly the 16 tables' columns" ||
  ko "qd_grafana's grants differ: $(printf '%s\n%s\n' "$want" "$have" | sort | uniq -u | tr '\n' ';')"
tables=$(echo "SELECT count(*) FROM information_schema.role_table_grants WHERE grantee = 'qd_grafana'" | as_super_pg)
[ "$tables" = 0 ] && ok "qd_grafana has no table-level privilege" || ko "qd_grafana's table-level privileges: $tables"
reads=$(echo "$granted" | while read -r t cols; do
  out=$(echo "SELECT count(*) FROM (SELECT $cols FROM $t) s" | as_grafana_pg)
  case "$out" in [0-9]*) echo "[OK  ] qd_grafana reads $t ($out rows)" ;; *) echo "[FAIL] qd_grafana reads $t: $out" ;; esac
done)
echo "$reads"
fails=$((fails + $(echo "$reads" | grep -c FAIL)))
for q in "SELECT parameters FROM quality.jobs" "SELECT * FROM quality.jobs" "SELECT 1 FROM public.secrets" \
  "SELECT 1 FROM public.admin_sessions" "SELECT 1 FROM quality.console_results" "SELECT 1 FROM quality.source_records" \
  "SELECT 1 FROM public.tenants" "SELECT password_hash FROM public.admin_users"; do
  case "$(echo "$q LIMIT 1" | as_grafana_pg)" in
    *"permission denied"*) ok "denied: $q" ;;
    *) ko "not denied: $q" ;;
  esac
done
# Writes are refused by the grants, not only by the read-only default: it is
# turned off first, and every statement is rolled back whatever happens.
for q in "INSERT INTO quality.remediation_slas (dataset, dimension, days) VALUES ('probe', 'probe', 1)" \
  "UPDATE quality.jobs SET priority = priority WHERE false" "DELETE FROM quality.cells WHERE false" \
  "CREATE TABLE quality.qd_grafana_probe (x int)" "CREATE TABLE public.qd_grafana_probe (x int)"; do
  out=$(printf 'SET default_transaction_read_only = off;\nBEGIN;\nSHOW transaction_read_only;\n%s;\nROLLBACK;\n' "$q" | as_grafana_pg)
  case "$out" in
    "off"*"permission denied"*) ok "denied with writes allowed: ${q%% (*}" ;;
    *) ko "write: $q: $out" ;;
  esac
done
# The tenant: the one open tenant (or QD_GRAFANA_TENANT), and the policies
# hide another's rows (one made and rolled back by the superuser, read as
# qd_grafana with its tenant).
tenant=$(printf "SELECT id FROM public.tenants WHERE status <> 'closed' AND (:'t' = '' OR id::text = :'t' OR slug = :'t')\n" |
  docker exec -i "$pg" psql -X -At -U "${POSTGRES_SUPERUSER:-postgres}" -d "$db" --set t="${QD_GRAFANA_TENANT:-}")
seen=$(echo "SELECT current_setting('app.tenant_id') || ' ' || public.current_tenant()" | as_grafana_pg)
[ "$seen" = "$tenant $tenant" ] && ok "qd_grafana's tenant: $tenant" || ko "qd_grafana's tenant: $seen (expected $tenant)"
other=$(printf "BEGIN;
INSERT INTO quality.remediation_slas (tenant_id, dataset, dimension, days) VALUES (gen_random_uuid(), 'probe', 'probe', 1);
SET LOCAL app.tenant_id = '%s';
SET LOCAL ROLE qd_grafana;
SELECT count(*) FROM quality.remediation_slas WHERE dataset = 'probe' AND dimension = 'probe';
ROLLBACK;\n" "$tenant" | as_super_pg)
[ "$other" = 0 ] && ok "another tenant's row is not visible to qd_grafana" || ko "another tenant's row: $other"
attrs=$(echo "SELECT rolsuper, rolbypassrls, rolinherit, rolcreaterole, rolcreatedb, rolreplication, rolconnlimit FROM pg_roles WHERE rolname = 'qd_grafana'" | as_super_pg)
[ "$attrs" = "f|f|f|f|f|f|10" ] && ok "qd_grafana: no superuser, no bypassrls, no inherit, 10 connections" || ko "qd_grafana's attributes: $attrs"

# 12. Grafana's PostgreSQL datasource (provisioning/datasources/postgres.yml):
# healthy, read-only, its password stored encrypted from the file, a query
# answered, an ungranted column refused, and the password nowhere in its
# configuration.
health=$(as_admin "http://127.0.0.1:$gport/api/datasources/uid/postgres/health")
case "$health" in *'"status":"OK"'*) ok "postgres datasource health" ;; *) ko "postgres datasource health: $health" ;; esac
ds=$(as_admin "http://127.0.0.1:$gport/api/datasources/uid/postgres")
case "$ds" in
  *'"type":"grafana-postgresql-datasource"'*'"user":"qd_grafana"'*'"secureJsonFields":{"password":true}'*'"readOnly":true'*)
    ok "postgres datasource: qd_grafana, password encrypted, read-only" ;;
  *) ko "postgres datasource: $ds" ;;
esac
echo "$ds" | grep -qF -f "$secrets/grafana_pg_password" && ko "the role's password in the datasource's API" ||
  ok "no password in the datasource's API"
ds_query() {
  as_admin -H 'Content-Type: application/json' -X POST "http://127.0.0.1:$gport/api/ds/query" \
    -d "{\"queries\":[{\"refId\":\"A\",\"datasource\":{\"uid\":\"postgres\"},\"format\":\"table\",\"rawSql\":\"$1\"}]}"
}
case "$(ds_query 'SELECT kind, state, count(*) AS jobs FROM quality.jobs GROUP BY 1, 2')" in
  *'"error"'*) ko "query through grafana: $(ds_query 'SELECT kind, state, count(*) FROM quality.jobs GROUP BY 1, 2' | head -c 300)" ;;
  *'"status":200'*'"frames"'*) ok "query through grafana: jobs by kind and state" ;;
  *) ko "query through grafana: no frame" ;;
esac
case "$(ds_query 'SELECT parameters FROM quality.jobs LIMIT 1')" in
  *'permission denied'*) ok "grafana refused quality.jobs.parameters" ;;
  *) ko "grafana read quality.jobs.parameters" ;;
esac
if docker exec "$grafana" sh -c 'cat /etc/grafana/provisioning/datasources/*.yml' | grep -qF -f "$secrets/grafana_pg_password"; then
  ko "the role's password in grafana's provisioning"
else
  ok "the role's password only in its secret file"
fi

# 13. The dashboards (point 8), generated by tools/grafana/dashboards.mjs:
# each file's dashboard is listed in the folder named by its directory and
# provisioned, and the API refuses to save it as is (an admin's meta.canSave
# is true even so); then check-queries.mjs runs every panel's query through
# Traefik, with the dev CA, the password read from its file. Counts only.
field() { node -e 'let s="";process.stdin.on("data",(d)=>(s+=d)).on("end",()=>{let v=JSON.parse(s);
  for (const k of process.argv[1].split(".")) v=Array.isArray(v)?v.find((x)=>x.uid===k):v?.[k];
  console.log(typeof v==="object"?JSON.stringify(v):String(v))})' "$1"; }
expected=$(cd infra/observability/grafana/dashboards && for f in */*.json; do
  printf '%s\t%s\n' "$(field uid <"$f")" "${f%/*}"; done)
search=$(as_admin "http://127.0.0.1:$gport/api/search?type=dash-db")
dashes=$(echo "$expected" | while IFS="$(printf '\t')" read -r uid folder; do
  [ "$(echo "$search" | field "$uid.folderTitle")" = "$folder" ] &&
    echo "[OK  ] $uid listed in «$folder»" || echo "[FAIL] $uid not listed in «$folder»"
  got=$(as_admin "http://127.0.0.1:$gport/api/dashboards/uid/$uid")
  [ "$(echo "$got" | field meta.provisioned)" = true ] && echo "[OK  ] $uid provisioned" || echo "[FAIL] $uid not provisioned"
  body=$(echo "$got" | node -e 'let s="";process.stdin.on("data",(d)=>(s+=d)).on("end",()=>{const d=JSON.parse(s);
    console.log(JSON.stringify({dashboard:d.dashboard,folderUid:d.meta.folderUid,overwrite:false}))})')
  saved=$(as_admin -H 'Content-Type: application/json' -X POST "http://127.0.0.1:$gport/api/dashboards/db" --data-binary "$body")
  case "$saved" in *'Cannot save provisioned dashboard'*) echo "[OK  ] $uid not saveable" ;;
    *) echo "[FAIL] $uid saved: $(echo "$saved" | head -c 200)" ;; esac
done)
echo "$dashes"
fails=$((fails + $(echo "$dashes" | grep -c FAIL)))
want=$(for slug in audit personal-data runs run-logs health quality; do for lang in es en; do
  echo "qd-$slug-$lang"; done; done | sort)
have=$(echo "$expected" | cut -f1 | sort)
[ "$have" = "$want" ] && ok "$(echo "$want" | grep -c .) generated dashboards, those expected" ||
  ko "generated dashboards differ from those expected: $(echo $have)"
queries=$(NODE_EXTRA_CA_CERTS=./infra/traefik/certs/dev-ca.crt GRAFANA_URL="https://$domain/grafana" \
  node tools/grafana/check-queries.mjs --password-file "$secrets/grafana_admin_password" 2>&1)
echo "$queries" | grep -v '^\[OK' | sed 's/^/       /'
echo "$queries" | tail -1 | grep -q '^all queries of' && ok "check-queries: $(echo "$queries" | grep -c '^\[OK') panel queries ran" ||
  ko "check-queries: $(echo "$queries" | grep -c '^\[FAIL') failed"

# 14. The level label (alloy/config.alloy): a probe container in a project
# Alloy collects writes one synthetic line in each JSON shape the services
# use (the Rust services' `INFO`, Dolt's `warning`, Trino's `WARN`, Traefik's
# access log), one with a level outside the set and one of text; PostgreSQL
# writes an error (a division by zero). Then each service's recent lines:
# labelled when they are JSON or PostgreSQL's; a service still on text until
# its next start is only reported.
loki() { inside -G "http://127.0.0.1:${LOKI_HTTP_PORT:-3100}/loki/api/v1/query_range" --data-urlencode "query=$1" \
  --data-urlencode "limit=${2:-50}" --data-urlencode "since=${3:-10m}"; }
# level<TAB>line of a query's result, newest first.
levels() { node -e 'let s="";process.stdin.on("data",(d)=>(s+=d)).on("end",()=>{const r=JSON.parse(s).data?.result??[];
  r.flatMap((x)=>x.values.map((v)=>[v[0],x.stream.level??"-",v[1]])).sort((a,b)=>(a[0]<b[0]?1:-1))
   .forEach((v)=>console.log(v[1]+"\t"+v[2].slice(0,120)))})'; }
mark=qd-level-probe-$(date +%s)
docker run -d --rm --label com.docker.compose.project=data-quality-verify --label com.docker.compose.service=probe \
  -e MARK="$mark" alpine:3.20 sh -c 'sleep 20
for l in "{\"time\":\"t\",\"level\":\"INFO\",\"service\":\"server\",\"msg\":\"$MARK rust info\"}" \
  "{\"level\":\"warning\",\"msg\":\"$MARK dolt warn\",\"time\":\"t\"}" \
  "{\"timestamp\":\"t\",\"level\":\"WARN\",\"logger\":\"io.trino\",\"message\":\"$MARK trino warn\"}" \
  "{\"DownstreamStatus\":200,\"RouterName\":\"probe@file\",\"level\":\"info\",\"msg\":\"$MARK traefik info\"}" \
  "{\"level\":\"ERROR\",\"msg\":\"$MARK rust error\"}" \
  "{\"level\":\"verbose\",\"msg\":\"$MARK other -\"}" "$MARK text -"; do echo "$l"; done
sleep 30' >/dev/null
echo "SELECT 1/0" | as_super_pg >/dev/null
sleep "${PROBE_WAIT:-50}"
got=$(loki "{compose_project=\"data-quality-verify\"} |= \"$mark\"" | levels)
for want in "rust info" "dolt warn" "trino warn" "traefik info" "rust error" "other -" "text -"; do
  level=$(echo "$got" | grep -F "$mark $want" | cut -f1)
  [ "$level" = "${want##* }" ] && ok "level of the ${want% *} probe: $level" || ko "level of the ${want% *} probe: ${level:-no line}"
done
level=$(loki '{service="postgres"} |= "ERROR:  division by zero"' 1 2m | levels | cut -f1)
[ "$level" = error ] && ok "level of postgres's ERROR: error" || ko "level of postgres's ERROR: ${level:-no line}"
values=$(inside "http://127.0.0.1:${LOKI_HTTP_PORT:-3100}/loki/api/v1/label/level/values" |
  node -e 'let s="";process.stdin.on("data",(d)=>(s+=d)).on("end",()=>console.log((JSON.parse(s).data??[]).join(" ")))')
outside=$(echo "$values" | tr ' ' '\n' | grep -vxE 'trace|debug|info|warn|error|fatal' | tr '\n' ' ')
[ -n "$values" ] && [ -z "$outside" ] && ok "level values within the set: $values" || ko "level values: $values (outside: $outside)"
for s in 'data-quality traefik' 'data-quality postgres' 'data-quality-dolt dolt' 'data-quality-trino coordinator' \
  'data-quality-trino worker' 'data-quality-observability grafana' 'data-quality-observability loki'; do
  project=${s% *} service=${s#* }
  last=$(loki "{compose_project=\"$project\", service=\"$service\"}" 1 1h | levels)
  if [ -n "$(loki "{compose_project=\"$project\", service=\"$service\", level=~\".+\"}" 1 1h | levels)" ]; then
    ok "$service: lines with a level"
  elif [ -z "$last" ]; then
    echo "[INFO] $service: no line in the last hour"
  else
    case "$(echo "$last" | cut -f2)" in
      '{'*) ko "$service: JSON lines without a level" ;;
      *) echo "[INFO] $service: still text, JSON from its next start" ;;
    esac
  fi
done

# 15. OBS_VM reaches the Rust services (decision 0026, point 1): the API in
# the dev and the production compose files, the worker in its own; the
# rendered configuration is read for that one value only (it carries
# secrets).
vm_of() { node -e 'let s="";process.stdin.on("data",(d)=>(s+=d)).on("end",()=>{
  console.log(JSON.parse(s).services?.[process.argv[1]]?.environment?.OBS_VM ?? "missing")})' "$1"; }
dc() { docker compose --project-directory . --env-file .env "$@" config --format json 2>/dev/null; }
for c in "api|dev api|--profile dev -f compose.yml -f compose.override.yml" \
  "api|production api|--profile prod -f compose.yml -f compose.prod.yml -f compose.local.yml" \
  "worker|worker|-p data-quality-worker -f infra/worker/docker-compose.yml"; do
  service=${c%%|*} rest=${c#*|}
  # shellcheck disable=SC2086
  got=$(OBS_VM=qd-vm-probe API_IMAGE=probe WEB_IMAGE=probe dc ${rest#*|} | vm_of "$service")
  [ "$got" = qd-vm-probe ] && ok "OBS_VM in the ${rest%%|*}'s environment" || ko "OBS_VM in the ${rest%%|*}'s environment: $got"
done

# 16. The API's own lines (alloy/dev-host.alloy in watch mode, the
# container's output under `task dev`): a GET of /api/health, its request id
# from the response's x-request-id, is found in Loki by that JSON field
# under the labels the dashboards use, with a level. Prints the line's field
# names only.
rid=$(curl -sk -D - -o /dev/null "https://$domain/api/health" | tr -d '\r' | sed -n 's/^x-request-id: //Ip')
if [ -z "$rid" ]; then
  ko "api: no x-request-id from /api/health"
else
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    got=$(loki "{service=\"api\", compose_project=\"data-quality\"} | json | request_id=\"$rid\"" 5 5m)
    echo "$got" | grep -q '"values":\[\[' && break
    sleep 3
  done
  stream=$(echo "$got" | node -e 'let s="";process.stdin.on("data",(d)=>(s+=d)).on("end",()=>{const r=JSON.parse(s).data?.result??[];
    if(!r.length)return;const st=r[0].stream;const keys=Object.keys(JSON.parse(r[0].values[0][1])).sort();
    console.log(`vm=${st.vm??"-"} level=${st.level??"-"}\t${keys.join(",")}`)})')
  case "$stream" in
    "vm="?*" level="[a-z]*"	"*request_id*) ok "api's line of request $rid in loki: ${stream%%	*}, fields ${stream#*	}" ;;
    "") ko "api's line of request $rid not in loki under {service=\"api\", compose_project=\"data-quality\"}" ;;
    *) ko "api's line of request $rid: $stream" ;;
  esac
fi

# 17. A host process's text (alloy/dev-host.alloy, watch mode only): a line
# of tracing's text format, in colour, as a worker built before it used the
# crate logging writes, put in a file of the worker*.log pattern of its own,
# reaches Loki without the colours and with the level label; the file is
# removed after. Waits for Alloy to find the new file (sync_period, 15 s).
if [ -d .dev/logs ]; then
  probe=.dev/logs/worker-probe$(date +%s).log
  mark=qd-text-probe-$(date +%s)
  : >"$probe" && chmod 644 "$probe"
  sleep "${PROBE_WAIT_FILE:-20}"
  esc=$(printf '\033')
  printf '%s[2m%s%s[0m %s[33m WARN%s[0m qd_worker: %s text\n' "$esc" "$(date -u +%Y-%m-%dT%H:%M:%S.000000Z)" \
    "$esc" "$esc" "$esc" "$mark" >>"$probe"
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    got=$(loki "{service=\"worker\", compose_project=\"data-quality-worker\"} |= \"$mark\"" 5 5m | levels)
    [ -n "$got" ] && break
    sleep 3
  done
  rm -f "$probe"
  case "$got" in
    *"$esc"*) ko "a host worker's text line keeps its colours in loki" ;;
    "warn	"*"$mark text"*) ok "a host worker's text line in loki: level warn, no colours" ;;
    "") ko "a host worker's text line not in loki" ;;
    *) ko "a host worker's text line: level $(echo "$got" | cut -f1)" ;;
  esac
else
  echo "[INFO] no .dev/logs: not in watch mode, the host's text probe is skipped"
fi

# 18. Logs by run (point 8): the latest cut's lines are found in Loki by its
# run_id, the JSON field, and by the run-logs dashboard's line filter (the
# cut's id or one of its jobs' ids), over the run's own time span. A real
# check once a worker's lines reached Loki during that run; with no cut, or
# a run older than that (no worker line in Loki then), only reported.
# Prints ids, counts and field names, never a line.
loki_span() { inside -G "http://127.0.0.1:${LOKI_HTTP_PORT:-3100}/loki/api/v1/query_range" --data-urlencode "query=$1" \
  --data-urlencode "limit=1000" --data-urlencode "start=${2}000000000" --data-urlencode "end=${3}000000000"; }
# lines<TAB>services<TAB>fields of a query's result (fields of its JSON lines).
summary() { node -e 'let s="";process.stdin.on("data",(d)=>(s+=d)).on("end",()=>{const r=JSON.parse(s).data?.result??[];
  const sv=new Set(),f=new Set();let n=0;for(const x of r){sv.add(x.stream.service);for(const v of x.values){n++;
  try{Object.keys(JSON.parse(v[1])).forEach((k)=>f.add(k))}catch{}}}
  console.log(n+"\t"+[...sv].sort().join(",")+"\t"+[...f].sort().join(","))})'; }
last=$(echo "SELECT c.id, extract(epoch FROM c.fired_at)::bigint - 60,
    extract(epoch FROM coalesce(max(j.finished_at), now()))::bigint + 300,
    coalesce(string_agg(j.id::text, ','), '')
  FROM quality.cuts c LEFT JOIN quality.jobs j ON j.id IN (c.extract_job, c.evaluate_job)
  GROUP BY c.id, c.fired_at ORDER BY c.fired_at DESC LIMIT 1" | as_grafana_pg)
if [ -z "$last" ]; then
  echo "[INFO] logs by run: no cut yet"
else
  IFS='|' read -r run from to jobs <<EOF
$last
EOF
  workers=$(loki_span '{service="worker", compose_project="data-quality-worker"}' "$from" "$to" | summary | cut -f1)
  by_field=$(loki_span "{compose_project=~\"data-quality.*\"} | json | run_id=\"$run\"" "$from" "$to" | summary)
  by_text=$(loki_span "{compose_project=~\"data-quality.*\", service!~\"grafana|loki|alloy\"} |~ \`$run|$(echo "$jobs" | tr ',' '|')\`" \
    "$from" "$to" | summary | cut -f1)
  n=${by_field%%	*}
  when=$(date -u -d "@$((from + 60))" +%Y-%m-%dT%H:%MZ)
  if [ "${workers:-0}" = 0 ]; then
    echo "[INFO] logs by run: latest run $run (fired $when) has no worker line in loki in its span: not checked"
  elif [ "${n:-0}" -gt 0 ] && [ "${by_text:-0}" -gt 0 ]; then
    fields=$(echo "$by_field" | cut -f3 | tr ',' '\n' | grep -xE 'job_id|run_id|run|trino_query_id|request_id' | tr '\n' ',')
    ok "logs by run $run (fired $when): $n lines by run_id ($(echo "$by_field" | cut -f2)), $by_text by the dashboard's filter; ids: ${fields%,}"
  else
    ko "logs by run $run (fired $when): workers wrote $workers lines then, ${n:-0} by run_id, ${by_text:-0} by the dashboard's filter"
  fi
fi

# 19. The services' metrics (decision 0071, step 2; alloy/app.alloy): in
# Mimir, `up` is 1 for each target development collects (PostgreSQL only
# with certificate.yml), and no series of this VM has a metric or a label
# name outside the keep lists. metrics_app_cert.sh checks each target's
# lists apart, the limits and the installation's hops.
mimir_api() { api=$1; shift; inside -G "http://127.0.0.1:${MIMIR_HTTP_PORT:-9009}/prometheus/api/v1/$api" "$@"; }
vm=${OBS_VM:-dev}
for _ in 1 2 3 4 5 6 7 8; do
  ups=$(mimir_api query --data-urlencode "query=up{vm=\"$vm\"} == 1" | node -e 'let s="";process.stdin.on("data",(d)=>(s+=d)).on("end",()=>
    console.log((JSON.parse(s).data?.result??[]).map((r)=>r.metric.job).sort().join(" ")))' 2>/dev/null)
  [ "$ups" = "alloy dolt grafana loki mimir traefik" ] && break
  sleep 10
done
[ "$ups" = "alloy dolt grafana loki mimir traefik" ] && ok "metrics: up == 1 in mimir for $ups" ||
  ko "metrics: up == 1 for '${ups:-none}', expected alloy dolt grafana loki mimir traefik"
names="up process_cpu_seconds_total process_resident_memory_bytes traefik_service_requests_total traefik_entrypoint_requests_total
  traefik_tls_certs_not_after dss_concurrent_connections dss_concurrent_queries dss_connects dss_disconnects
  loki_distributor_lines_received_total loki_ingester_chunks_flushed_total cortex_distributor_received_samples_total
  cortex_ingester_shipper_uploads_total cortex_discarded_samples_total loki_write_sent_entries_total
  loki_write_dropped_entries_total prometheus_remote_storage_samples_failed_total"
for h in traefik_service_request_duration_seconds traefik_entrypoint_request_duration_seconds \
  grafana_http_request_duration_seconds dss_query_duration loki_request_duration_seconds; do
  names="$names ${h}_bucket ${h}_sum ${h}_count"
done
labels="__name__ job instance vm role code method protocol service entrypoint le cn sans handler status_code
  route ws reason group component_id host remote_name url"
outside=$(mimir_api series --data-urlencode "match[]={vm=\"$vm\"}" --data-urlencode "start=$(($(date +%s) - 900))" |
  node -e 'let s="";process.stdin.on("data",(d)=>(s+=d)).on("end",()=>{const [n,l]=process.argv.slice(1).map((x)=>new Set(x.split(/\s+/)));
    const out=new Set();for(const m of JSON.parse(s).data??[]){if(!n.has(m.__name__))out.add(m.__name__);
    for(const k of Object.keys(m))if(!l.has(k))out.add("label "+k)}console.log([...out].sort().join(", ")||"none")})' "$names" "$labels")
[ "$outside" = none ] && ok "metrics: every series of vm=$vm within the keep lists" || ko "metrics: outside the keep lists: $outside"

[ "$fails" = 0 ] && echo "all checks passed" || { echo "$fails check(s) failed"; exit 1; }
