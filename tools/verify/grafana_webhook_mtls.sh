#!/bin/bash
# Verification V3 of decision 0072, brought to step 5's files and route:
# Grafana 13.2.5 provisioned with the repository's
# infra/observability/grafana/provisioning (alerting/*.yml as alerts.mjs
# generates them, the datasources) through grafana/tls/entrypoint.sh, as
# infra/observability/certificate.yml runs it, which adds the application
# VM's certificate to every webhook; the repository's Traefik files
# (traefik.yml, internal.yml and middlewares.yml) in front of the real API
# (this checkout's `server`, run on this machine, never built here) with
# INTERNAL_CLIENT_CERT_HEADER=trusted, its CA from `server ca-init` and the
# application VM's machine from `server app-machine`, against a throwaway
# PostgreSQL 16; a throwaway Mimir 3.2.4 serving and requiring certificates
# of that CA, as Grafana's datasource reads it.
#
# It shows: the route through Traefik's internal entry point refusing no
# certificate and another CA's at the handshake, answering 403 to an
# approved worker's certificate and 200 to the application VM's, an alert of
# no known machine counted in the API's metrics file; both webhooks of the
# contact points with their TLS from the entrypoint (shown redacted by
# Grafana's API); Grafana's heartbeat delivered with the certificate and
# kept as the heartbeat's row of machine_alerts; and Grafana provisioned
# with the same files without the entrypoint (the CA trusted, no client
# certificate) refused at the handshake, the row unchanged. The intervals
# are the files' (the heartbeat's first delivery within some 2 minutes), so
# it runs some 8 minutes. The 2026-11-16 run's other checks (payloads,
# fingerprints, the certificate replaced, retries, order; against a sink,
# with grafana_webhook_runs.sh, now removed) stay in V3's record and in git.
#
# Containers qd-verify-webhook-…, network and volumes of that name, removed
# on exit; PostgreSQL on 127.0.0.1:15441, the API on the network's gateway,
# port 18091; nothing of the dev stack is touched, no secret printed. Run it
# alone.
#
#   bash tools/verify/grafana_webhook_mtls.sh   [SERVER=target/debug/server]
P=qd-verify-webhook
. "$(dirname "$0")/machines_lib.sh"
. "$(dirname "$0")/grafana_alerting_lib.sh"
SERVER=${SERVER:-${CARGO_TARGET_DIR:-$ROOT/target}/debug/server}
[ -x "$SERVER" ] || { echo "no server at $SERVER: cargo build -p server"; exit 2; }
"$SERVER" openapi 2>/dev/null | grep -q '/internal/machines/alerts' ||
  { echo "$SERVER has no alerts route: build it again"; exit 2; }
PGPORT=15441
APIPORT=18091
API_PID=
eval "$(declare -f cleanup | sed '1s/cleanup/lib_cleanup/')"
cleanup() { [ -n "$API_PID" ] && kill "$API_PID" 2>/dev/null; lib_cleanup; }
start_run

echo "== The installation: PostgreSQL, the CA, the application VM's machine, the API"
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
mkdir -m 755 "$D/node" "$D/metrics" "$D/settings"
srv() { env -i PATH="$PATH" HOME="$D" DATABASE_URL_FILE="$D/database_url" CA_CERT_FILE="$D/ca.crt" CA_KEY_FILE="$D/ca.key" "$@"; }
note "app-machine: $(srv "$SERVER" app-machine --name qd-app --dir "$D/node" 2>&1 | tail -1 | cut -c1-120)"
[ -s "$D/node/machine.pem" ] || { ko "server app-machine wrote no machine.pem"; exit 1; }
# Readable by the containers' users: this test's own throwaway key.
install -m 644 "$D/node/machine.pem" "$D/app.pem"
GW=$(docker network inspect "$NET" -f '{{(index .IPAM.Config 0).Gateway}}')
# Not through srv: $! must be the server's own PID for the cleanup to end it.
env -i PATH="$PATH" HOME="$D" DATABASE_URL_FILE="$D/database_url" CA_CERT_FILE="$D/ca.crt" CA_KEY_FILE="$D/ca.key" \
  APP_HOST="$GW" APP_PORT=$APIPORT SETTINGS_KEY_FILE="$D/settings/settings.key" OBS_VM=verify-app \
  INTERNAL_CLIENT_CERT_HEADER=trusted QD_METRICS_DIR="$D/metrics" RUST_LOG=warn "$SERVER" serve >"$D/api.log" 2>&1 &
API_PID=$!
for _ in $(seq 60); do [ "$(curl -s -o /dev/null -w '%{http_code}' "http://$GW:$APIPORT/health")" = 200 ] && break; sleep 2; done
expect "the API answers /health" 200 "$(curl -s -o /dev/null -w '%{http_code}' "http://$GW:$APIPORT/health")"
# An approved worker with a certificate of the CA, its serial as the API keeps it.
openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "/O=qd/OU=worker/CN=verify-worker" \
  -keyout "$D/worker.k" -out "$D/worker.csr" 2>/dev/null
printf 'basicConstraints=CA:FALSE\nextendedKeyUsage=clientAuth\nsubjectAltName=DNS:verify-worker\n' >"$D/ext"
openssl x509 -req -in "$D/worker.csr" -CA "$D/ca.crt" -CAkey "$D/ca.key" -set_serial 0x7a11ce0001 \
  -extfile "$D/ext" -days 1 -out "$D/worker.crt" 2>/dev/null
openssl pkey -in "$D/worker.k" -out "$D/worker.key" && cat "$D/worker.crt" "$D/worker.key" >"$D/worker.pem"
chmod 644 "$D/worker.pem"
sql "INSERT INTO machines (name, role, state, cert_serial, cert_not_after) VALUES \
  ('verify-worker', 'worker', 'approved', '7a11ce0001', now() + interval '1 day')" >/dev/null
ca other
cert stranger other "/O=qd/OU=app/CN=qd-app" qd-app
# Mimir as certificate.yml runs it: HTTPS with a certificate of the CA, a client's required.
cert mimirsrv ca "/O=qd/OU=app/CN=qd-app" "qd-app,DNS:mimir"
MIMIR_FLAGS="-server.http-tls-cert-path=/c/mimirsrv.pem -server.http-tls-key-path=/c/mimirsrv.pem -server.http-tls-ca-path=/c/ca.crt -server.http-tls-client-auth=RequireAndVerifyClientCert"
MIMIR_TLS=1 MIMIR_CERT=app
export MIMIR_TLS MIMIR_CERT
mimir_up || exit 1

echo "== Traefik with the repository's files in front of the API"
DYN=$D/dynamic
mkdir -p "$DYN"
cp "$ROOT/infra/traefik/internal.yml" "$ROOT/infra/traefik/dynamic/middlewares.yml" "$DYN/"
cp "$D/ca.crt" "$DYN/machines-ca.crt" && cp "$D/app.pem" "$DYN/app-machine.pem"
printf 'http:\n  services:\n    api:\n      loadBalancer:\n        servers:\n          - url: "http://%s:%s"\n' "$GW" $APIPORT >"$DYN/api.yml"
chmod 755 "$DYN" && chmod 644 "$DYN"/*
docker run -d --name "$P-traefik" --network "$NET" --network-alias qd-app --tmpfs /letsencrypt \
  -e INTERNAL_DOMAIN=qd-app -e INTERNAL_API_SERVICE=api@file \
  -e INTERNAL_CERT_FILE=/etc/traefik/dynamic/app-machine.pem -e INTERNAL_KEY_FILE=/etc/traefik/dynamic/app-machine.pem \
  -v "$ROOT/infra/traefik/traefik.yml:/etc/traefik/traefik.yml:ro" -v "$DYN:/etc/traefik/dynamic:ro" \
  traefik:v3.6 --configFile=/etc/traefik/traefik.yml >/dev/null
URL=https://qd-app:9443/api/internal/machines/alerts
hit() { # CERT|none BODY: the answer's body and status (000: no TLS session)
  local c=()
  [ "$1" != none ] && c=(--cert "/c/$1.pem")
  docker run --rm --network "$NET" -v "$D:/c:ro" "$CURL" -s -w ' %{http_code}' -m 10 --cacert /c/ca.crt \
    "${c[@]}" -H 'Content-Type: application/json' -X POST -d "$2" "$URL"
}
status() { hit "$@" | awk '{ print $NF }'; }
UNKNOWN='{"alerts":[{"status":"firing","fingerprint":"0a1b2c3d4e5f6789","startsAt":"2026-11-17T10:00:00Z","labels":{"alertname":"disk_low","qd_alert":"disk_low","severity":"warning","vm":"verify-gone"},"values":{"A":0.07,"C":1}}]}'
for _ in $(seq 30); do [ "$(status app '{"alerts":[]}')" = 200 ] && break; sleep 2; done
expect "no certificate: refused at the handshake" 000 "$(status none '{"alerts":[]}')"
expect "another CA's certificate: refused at the handshake" 000 "$(status stranger '{"alerts":[]}')"
expect "an approved worker's certificate of the CA: refused by the API" "403 certificate_refused" \
  "$(hit worker "$UNKNOWN" | python3 -I -c 'import json, sys; b, s = sys.stdin.read().rsplit(" ", 1); print(s, json.loads(b)["code"])')"
expect "the application VM's certificate, an alert of no known machine: taken, nothing kept" '{"applied":0,"unknown":1} 200' \
  "$(hit app "$UNKNOWN")"
expect "machine_alerts after both" 0 "$(sql 'SELECT count(*) FROM machine_alerts')"
for _ in $(seq 20); do grep -q '^qd_machine_alerts_unknown_total 1' "$D/metrics/api.prom" 2>/dev/null && break; sleep 2; done
expect "qd_machine_alerts_unknown_total in the API's file" "qd_machine_alerts_unknown_total 1" \
  "$(grep '^qd_machine_alerts_unknown_total' "$D/metrics/api.prom" 2>/dev/null)"
expect "the API's log: one line naming the machine and qd_alert, no value" "1 0" \
  "$(grep 'verify-gone' "$D/api.log" | grep -c 'disk_low') $(grep -c '0\.07' "$D/api.log")"

echo "== Grafana with the repository's provisioning, through grafana/tls/entrypoint.sh"
mkdir -p "$D/gfnode" && cp "$D/node/ca.crt" "$D/node/machine.pem" "$D/gfnode/"
cp -R "$ROOT/infra/observability/grafana/provisioning" "$D/prov" && chmod -R a+rX "$D/prov" "$D/gfnode"
printf '%s' "$GF_PW" >"$D/gf_admin" && chmod 644 "$D/gf_admin"
# The PostgreSQL datasource's password (a secret of the compose): throwaway.
openssl rand -hex 12 >"$D/gf_pg" && chmod 644 "$D/gf_pg"
grafana() { # NAME [DOCKER OPTION...]: Grafana as the observability compose sets it, alias grafana
  local name=$1 i
  shift
  docker run -d --name "$P-$name" --network "$NET" --network-alias grafana -v "$P-$name-data:/var/lib/grafana" \
    -v "$D/gf_admin:/run/secrets/gf_admin:ro" -v "$D/gf_pg:/run/secrets/grafana_pg_password:ro" -v "$ROOT/infra/observability/grafana/dashboards:/var/lib/grafana-dashboards:ro" \
    -e GF_SECURITY_ADMIN_PASSWORD__FILE=/run/secrets/gf_admin -e GF_ANALYTICS_REPORTING_ENABLED=false \
    -e GF_ANALYTICS_CHECK_FOR_UPDATES=false -e GF_NEWS_NEWS_FEED_ENABLED=false -e GF_LOG_LEVEL=warn \
    -e MIMIR_URL=https://mimir:9009 -e MIMIR_SERVER_NAME=qd-app -e LOKI_URL=https://qd-app:3100 \
    -e GRAFANA_PG_HOST=qd-app -e GRAFANA_PG_PORT=5432 -e GRAFANA_PG_DATABASE=qd -e GRAFANA_PG_SSLMODE=verify-full \
    -e INTERNAL_DOMAIN=qd-app -e GRAFANA_ALERT_EMAILS=ops@example.org "$@" "$GRAFANA_IMAGE" >/dev/null
  for i in $(seq 90); do [ "$(gf /api/health -o /dev/null -w '%{http_code}')" = 200 ] && return 0; sleep 2; done
  note "Grafana not ready: $(docker logs "$P-$name" 2>&1 | tail -3 | cut -c1-300)"
  return 1
}
grafana grafana --user 0:0 --entrypoint /etc/qd/grafana/entrypoint.sh --tmpfs /run/qd-grafana:mode=0755 \
  -e GF_PATHS_PROVISIONING=/run/qd-grafana/provisioning -v "$D/gfnode:/run/qd-node:ro" -v "$D/prov:/etc/grafana/provisioning:ro" \
  -v "$ROOT/infra/observability/grafana/tls/entrypoint.sh:/etc/qd/grafana/entrypoint.sh:ro" || exit 1
expect "the entrypoint set the datasources' and webhooks' TLS" 1 \
  "$(docker logs "$P-grafana" 2>&1 | grep -c "datasources' and webhooks' TLS set")"
gf /api/v1/provisioning/contact-points >"$D/cps.json"
expect "both webhooks with their TLS, redacted (caCertificate, clientCertificate, clientKey)" \
  "qd-api REDACTED REDACTED REDACTED file|qd-api-heartbeat REDACTED REDACTED REDACTED file" \
  "$(python3 -I -c '
import json, sys
out = []
for c in sorted(json.load(open(sys.argv[1])), key=lambda c: c["uid"]):
    if c["type"] != "webhook": continue
    t = c["settings"].get("tlsConfig", {})
    v = ["REDACTED" if "REDACTED" in str(t.get(k)) else repr(t.get(k))[:12] for k in ("caCertificate", "clientCertificate", "clientKey")]
    out.append(" ".join([c["uid"], *v, c.get("provenance", "")]))
print("|".join(out))' "$D/cps.json")"
expect "the webhooks' URL, interpolated" "https://qd-app:9443/api/internal/machines/alerts" \
  "$(python3 -I -c 'import json, sys; print(" ".join(sorted({c["settings"]["url"] for c in json.load(open(sys.argv[1])) if c["type"] == "webhook"})))' "$D/cps.json")"
beat() { sql "SELECT count(*) || ' ' || coalesce(max(received_at)::text, '-') FROM machine_alerts WHERE machine_id IS NULL AND alertname = 'alerting_heartbeat'"; }
for _ in $(seq 60); do [ "$(beat | cut -d' ' -f1)" = 1 ] && break; sleep 5; done
first=$(beat)
expect "the heartbeat delivered with the certificate: the heartbeat's row, alone" "1 alerting_heartbeat alerting_heartbeat - -" \
  "$(sql "SELECT count(*), max(qd_alert), max(rule_name), coalesce(max(severity), '-'), coalesce(max(machine_id::text), '-') FROM machine_alerts" | tr '|' ' ')"
note "the heartbeat's received_at: ${first#* }"
expect "Grafana's datasource read Mimir with the certificate: no Datasource* row" 0 \
  "$(sql "SELECT count(*) FROM machine_alerts WHERE alertname LIKE 'Datasource%'")"

echo "== The same files without the entrypoint: the CA trusted, no client certificate"
docker rm -f "$P-grafana" >/dev/null
grafana plain -v "$D/prov:/etc/grafana/provisioning:ro" -v "$D/ca.crt:/etc/qd/ca.crt:ro" -e SSL_CERT_FILE=/etc/qd/ca.crt || exit 1
refused=0
for _ in $(seq 36); do
  docker logs "$P-plain" 2>&1 | grep -i 'qd-api-heartbeat' | grep -qi 'tls' && { refused=1; break; }
  sleep 5
done
expect "Grafana's delivery refused at the handshake (its log)" 1 "$refused"
note "Grafana said: $(docker logs "$P-plain" 2>&1 | grep -i 'qd-api-heartbeat' | grep -oi 'remote error: tls: [a-z ]*\|tls: [a-z ]*' | sort -u | head -2 | tr '\n' ';')"
expect "the heartbeat's row unchanged" "$first" "$(beat)"

echo "   Grafana 13.2.5, Traefik $(docker exec "$P-traefik" traefik version 2>/dev/null | awk '/Version/ { print $2; exit }'), Mimir 3.2.4, PostgreSQL 16"
echo "$PASSED passed, $FAILED failed"
[ $FAILED = 0 ] && echo "ALL OK" || echo "SOME FAILED"
exit $((FAILED > 0))
