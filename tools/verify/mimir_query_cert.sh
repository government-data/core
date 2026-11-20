#!/bin/bash
# Verification V4 of decision 0072, brought to step 7's route: GET
# /api/machines/metrics of this checkout's `server` (run on this machine,
# never built here), reading Mimir as an installation does since 0071's
# amendment of 2026-11-16: through the repository's Traefik files
# (traefik.yml, internal.yml and middlewares.yml; the `mimir` entry point,
# where only role `app` reads), presenting the application VM's machine
# certificate (`server app-machine`) as LAKE_CLIENT_CERT_FILE, with
# MIMIR_QUERY_URL. A throwaway PostgreSQL 16, the CA from `server ca-init`,
# a throwaway Mimir 3.2.4 with certificate.yml's TLS flags and, for this
# test alone, an out-of-order window of 25 h, a higher ingestion rate and
# the ingesters queried for any time, so that 24 hours of history can be
# pushed at once and read back (remote_write.py: 40 synthetic machines
# verify-00…39, a sample every 30 s, verify-07 with a two-hour gap from 10 h
# ago, verify-08 silent for the last 40 minutes); the machines approved in
# the API's table, verify-39 retired.
#
# It shows: the route's answer for 24 h and 1 h (available, step, points,
# the window on a step), only machines not retired, each machine's four
# charts, a gap as nulls, the last sample; a range outside the set 422,
# none without a session 401; a worker's certificate refused by Traefik; and
# with Mimir stopped the cached range still answered and another range
# `available: false` within the 5 s limit. The 2026-11-16 run's checks
# (Mimir's own answers to the five queries, sizes, alignment) stay in V4's
# record and in git.
#
# Containers qd-verify-mimirq-…, network of that name, removed on exit;
# PostgreSQL on 127.0.0.1:15442, Traefik's `mimir` entry point on
# 127.0.0.1:19446, the API on 127.0.0.1:18092; nothing of the dev stack is
# touched, no secret printed. Run it alone.
#
#   bash tools/verify/mimir_query_cert.sh   [SERVER=target/debug/server]
P=qd-verify-mimirq
. "$(dirname "$0")/machines_lib.sh"
SERVER=${SERVER:-${CARGO_TARGET_DIR:-$ROOT/target}/debug/server}
[ -x "$SERVER" ] || { echo "no server at $SERVER: cargo build -p server"; exit 2; }
"$SERVER" openapi 2>/dev/null | grep -q '/machines/metrics' ||
  { echo "$SERVER has no metrics route: build it again"; exit 2; }
PGPORT=15442
MPORT=19446
APIPORT=18092
API_PID=
eval "$(declare -f cleanup | sed '1s/cleanup/lib_cleanup/')"
cleanup() { [ -n "$API_PID" ] && kill "$API_PID" 2>/dev/null; lib_cleanup; }
start_run

echo "== The installation: PostgreSQL, the CA, the application VM's machine"
openssl rand -hex 16 >"$D/pg_password"
printf 'postgres://postgres:%s@127.0.0.1:%s/qd' "$(cat "$D/pg_password")" $PGPORT >"$D/database_url"
chmod 644 "$D/pg_password" && chmod 600 "$D/database_url"
docker run -d --name "$P-pg" --network "$NET" -p 127.0.0.1:$PGPORT:5432 -e POSTGRES_DB=qd \
  -e POSTGRES_PASSWORD_FILE=/run/pw -v "$D/pg_password:/run/pw:ro" postgres:16-alpine >/dev/null
sql() { docker exec "$P-pg" psql -XAtq -U postgres -d qd -c "$1"; }
for _ in $(seq 30); do sql 'select 1' >/dev/null 2>&1 && break; sleep 1; done
sql 'CREATE ROLE qd_queue LOGIN' >/dev/null
"$SERVER" ca-init --name verify --cert "$D/ca.crt" --key "$D/ca.key" >/dev/null 2>&1 || { ko "server ca-init"; exit 1; }
chmod 644 "$D/ca.crt" "$D/ca.key" # throwaway, read by openssl only
mkdir -m 755 "$D/node" "$D/settings"
srv() { env -i PATH="$PATH" HOME="$D" DATABASE_URL_FILE="$D/database_url" CA_CERT_FILE="$D/ca.crt" CA_KEY_FILE="$D/ca.key" "$@"; }
note "app-machine: $(srv "$SERVER" app-machine --name qd-app --dir "$D/node" 2>&1 | tail -1 | cut -c1-120)"
[ -s "$D/node/machine.pem" ] || { ko "server app-machine wrote no machine.pem"; exit 1; }
install -m 644 "$D/node/machine.pem" "$D/app.pem"
# Mimir's and Traefik's server certificate; a worker's, to be refused.
cert mimirsrv ca "/O=qd/OU=app/CN=localhost" "localhost,DNS:mimir"
cert worker ca "/O=qd/OU=worker/CN=verify-00" verify-00

echo "== Mimir, 24 hours of 40 machines pushed"
MIMIR_FLAGS="-server.http-tls-cert-path=/c/mimirsrv.pem -server.http-tls-key-path=/c/mimirsrv.pem -server.http-tls-ca-path=/c/ca.crt -server.http-tls-client-auth=RequireAndVerifyClientCert -querier.query-ingesters-within=0"
MIMIR_TLS=1 MIMIR_CERT=app
export MIMIR_TLS MIMIR_CERT
mimir_up '  out_of_order_time_window: 25h
  ingestion_rate: 1000000
  ingestion_burst_size: 20000000' || exit 1
cp "$(dirname "$0")/remote_write.py" "$D/" && chmod 644 "$D/remote_write.py"
NOW=$(( $(date +%s) / 30 * 30 - 60 ))
out=$(docker run --rm --network "$NET" -v "$D:/c:ro" python:3.12-slim python -I /c/remote_write.py \
  https://mimir:9009/api/v1/push /c/ca.crt /c/app.pem 40 "$NOW" 2>&1 | tail -1)
expect "the push" "pushed 40 machines" "$(echo "$out" | cut -d, -f1)"
expect "series in Mimir over the last hour (19 a machine)" 760 "$(scalar 'count(last_over_time({job="host"}[1h]))')"
for i in $(seq -w 0 39); do
  state=approved retired=NULL
  [ "$i" = 39 ] && state=retired retired='now()'
  sql "INSERT INTO machines (name, role, state, cert_serial, cert_not_after, retired_at) VALUES \
    ('verify-$i', 'worker', '$state', '7a11ce00$i', now() + interval '1 day', $retired)" >/dev/null
done
expect "machines in the API's table, verify-39 retired" "40 1" \
  "$(sql "SELECT count(*) FROM machines WHERE name LIKE 'verify-%'") $(sql "SELECT count(*) FROM machines WHERE state = 'retired'")"

echo "== Traefik with the repository's files in front of Mimir"
DYN=$D/dynamic
mkdir -p "$DYN"
cp "$ROOT/infra/traefik/internal.yml" "$ROOT/infra/traefik/dynamic/middlewares.yml" "$DYN/"
cp "$D/ca.crt" "$DYN/machines-ca.crt" && cp "$D/mimirsrv.pem" "$DYN/app-machine.pem"
chmod 755 "$DYN" && chmod 644 "$DYN"/*
docker run -d --name "$P-traefik" --network "$NET" -p 127.0.0.1:$MPORT:9446 --tmpfs /letsencrypt \
  -e INTERNAL_DOMAIN=localhost -e MIMIR_UPSTREAM=https://mimir:9009 \
  -e INTERNAL_CERT_FILE=/etc/traefik/dynamic/app-machine.pem -e INTERNAL_KEY_FILE=/etc/traefik/dynamic/app-machine.pem \
  -v "$ROOT/infra/traefik/traefik.yml:/etc/traefik/traefik.yml:ro" -v "$DYN:/etc/traefik/dynamic:ro" \
  traefik:v3.6 --configFile=/etc/traefik/traefik.yml >/dev/null
through() { # CERT: Traefik's status for a query with that certificate
  curl -s -o /dev/null -w '%{http_code}' -m 10 --cacert "$D/ca.crt" --cert "$D/$1.pem" \
    "https://localhost:$MPORT/prometheus/api/v1/query?query=up"
}
for _ in $(seq 30); do [ "$(through app)" = 200 ] && break; sleep 2; done
expect "through Traefik: the application VM's certificate reads, a worker's is refused" "200 403" \
  "$(through app) $(through worker)"

echo "== The API, MIMIR_QUERY_URL through Traefik with the application VM's certificate"
ADMIN_PASSWORD=Qd-$(openssl rand -hex 14)
apienv=(PATH="$PATH" HOME="$D" DATABASE_URL_FILE="$D/database_url" CA_CERT_FILE="$D/ca.crt" CA_KEY_FILE="$D/ca.key"
  APP_HOST=127.0.0.1 APP_PORT=$APIPORT SETTINGS_KEY_FILE="$D/settings/settings.key" OBS_VM=verify-app
  ADMIN_BOOTSTRAP_EMAIL=admin@example.com ADMIN_BOOTSTRAP_PASSWORD="$ADMIN_PASSWORD"
  MIMIR_QUERY_URL="https://localhost:$MPORT/prometheus" LAKE_CA_FILE="$D/ca.crt"
  LAKE_CLIENT_CERT_FILE="$D/node/machine.pem" RUST_LOG=warn)
env -i "${apienv[@]}" "$SERVER" serve >"$D/api.log" 2>&1 &
API_PID=$!
API=http://127.0.0.1:$APIPORT
for _ in $(seq 60); do [ "$(curl -s -o /dev/null -w '%{http_code}' "$API/health")" = 200 ] && break; sleep 2; done
expect "the API answers /health" 200 "$(curl -s -o /dev/null -w '%{http_code}' "$API/health")"
env -i "${apienv[@]}" "$SERVER" bootstrap-admin >/dev/null 2>&1
J=$D/headers
expect "the admin signs in" 200 "$(curl -s -o /dev/null -w '%{http_code}' -D "$J" -H 'content-type: application/json' \
  -d "{\"email\":\"admin@example.com\",\"password\":\"$ADMIN_PASSWORD\"}" "$API/auth/login")"
COOKIES=$(tr -d '\r' <"$J" | sed -n 's/^[Ss]et-[Cc]ookie: \([^;]*\).*/\1/p' | paste -sd';' | sed 's/;/; /g')
metrics() { # QUERY: the body, then a line with the status and the time
  curl -s -H "Cookie: $COOKIES" -w '\n%{http_code} %{time_total}' "$API/machines/metrics$1"
}
check() { # FILE: the answer's shape, one line
  python3 -I - "$1" <<'PY'
import json, sys
raw = open(sys.argv[1]).read().rsplit("\n", 1)
status, t = raw[1].split()
try:
    b = json.loads(raw[0])
except ValueError:
    b = None
if not isinstance(b, dict) or "machines" not in b:
    print(status, "no-body - - - - - - - - - - -", t); sys.exit()
ms = {m["name"]: m for m in b["machines"]}
n = (b["end"] - b["start"]) // b["step"] + 1
def whole(m):
    lines = [m["cpu"], m["memory"], m["network_in"], m["network_out"]] + [f["values"] for f in m["filesystems"]]
    return all(len(v) == n for v in lines) and sorted(f["label"] for f in m["filesystems"]) == ["docker", "spill"]
nulls = lambda vm: sum(v is None for v in ms.get(vm, {}).get("memory", [None]))
last = lambda vm: round((b["end"] - (ms.get(vm, {}).get("last_sample") or 0)) / 60)
print(status, b["available"], b["range"], b["step"], n, b["end"] % b["step"] == 0, len(ms),
      "verify-39" in ms, "qd-app" in ms, sum(1 for m in ms.values() if whole(m)), nulls("verify-00"),
      nulls("verify-07"), last("verify-08") - last("verify-00"), f"{float(t):.2f}")
PY
}
metrics '?range=24h' >"$D/day" && read -r -a day <<<"$(check "$D/day")"
note "24h: ${day[13]} s, $(head -1 "$D/day" | wc -c) bytes"
expect "24h: 200, available, a step of 10 min, 144 points, the window ending on a step" "200 True 24h 600 144 True" "${day[*]:0:6}"
expect "24h: 39 machines, not the retired verify-39, nor qd-app (no series)" "39 False False" "${day[*]:6:3}"
expect "24h: every machine with CPU, memory, network in and out and its two filesystems on every point" 39 "${day[9]}"
expect "24h: verify-00 no null in memory; verify-07's two-hour gap 11 or 12 nulls (the lookback filling one)" "0 yes" \
  "${day[10]} $([ "${day[11]}" -ge 11 ] 2>/dev/null && [ "${day[11]}" -le 12 ] && echo yes || echo "no (${day[11]})")"
expect "24h: verify-08's last sample 40 minutes before verify-00's" 40 "${day[12]}"
metrics '?range=1h' >"$D/hour" && read -r -a hour <<<"$(check "$D/hour")"
expect "1h: 200, available, a step of 1 min, 60 points, on a step, 39 machines" "200 True 1h 60 60 True 39" "${hour[*]:0:7}"
expect "a range outside 1h, 24h, 7d: 422" "422 422" \
  "$(metrics '?range=2h' | tail -1 | cut -d' ' -f1) $(metrics '?range=24h%20or%20up' | tail -1 | cut -d' ' -f1)"
expect "without a session: 401" 401 "$(curl -s -o /dev/null -w '%{http_code}' "$API/machines/metrics")"

echo "== Mimir stopped"
docker stop "$P-mimir" >/dev/null
metrics '?range=24h' >"$D/cached" && read -r -a cached <<<"$(check "$D/cached")"
expect "within 60 s the range asked before is answered from the API's cache" "200 True 39" "${cached[0]} ${cached[1]} ${cached[6]}"
metrics '?range=7d' >"$D/week" && read -r -a week <<<"$(check "$D/week")"
expect "another range: 200, available false, its window (168 points of 1 h), no machine" "200 False 7d 3600 168 True 0" "${week[*]:0:7}"
expect "and answered within the 5 s limit" yes "$(awk -v t="${week[13]}" 'BEGIN { print (t < 6) ? "yes" : "no (" t ")" }')"
note "the API's log lines about the series: $(grep -c 'series are not available' "$D/api.log")"
echo "   Mimir 3.2.4, $(docker run --rm traefik:v3.6 version 2>/dev/null | sed -n 's/^Version: *//p' | sed 's/^/Traefik /'), PostgreSQL 16, Python 3.12"
echo "$PASSED passed, $FAILED failed"
[ $FAILED = 0 ] && echo "ALL OK" || echo "SOME FAILED"
exit $((FAILED > 0))
