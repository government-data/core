#!/bin/bash
# Verification of decision 0071's amendment of 2026-11-16 (the user's
# ruling): reading Mimir is for the application VM's certificate (role
# `app`) alone, while every machine still pushes with its own. The
# repository's Traefik files as an installation runs them (traefik.yml, and
# internal.yml with middlewares.yml in its dynamic folder, rendered by
# Traefik with INTERNAL_DOMAIN qd-app and MIMIR_UPSTREAM) in front of a
# throwaway Mimir 3.2.4 with certificate.yml's TLS flags, a throwaway CA
# whose certificates are the CA's shape (O=qd, OU=<role>, CN=<name>).
#
# It shows, on Traefik's `mimir` entry point (9446, published as 9009 by
# infra/traefik/certificate.yml): who may read and with what status; that a
# client's own X-Forwarded-Tls-Client-Cert-Info does not pass the check, and
# (a router of this script's own) a rule cannot read the header at all; a path that leaves /api/v1/push; no certificate or another
# CA's refused at the handshake; the repository's worker.alloy pushing
# through it with a worker's, a lake node's and the application VM's
# certificates (each role's .alloy has the same remote write block); and
# Grafana's datasource as grafana/tls/entrypoint.sh provisions it, reading
# Mimir on the stack's network as before.
#
# Containers qd-verify-mimirrole-…, network of that name, removed on exit;
# no port published, no restart policy. Run it alone.
#
#   bash tools/verify/mimir_read_role.sh
P=qd-verify-mimirrole
. "$(dirname "$0")/machines_lib.sh"
TRAEFIK_IMAGE=traefik:v3.6
start_run
ca ca
ca other
# qd-app as Traefik and the clients name it; mimir as machines_lib.sh reaches it.
cert app ca "/O=qd/OU=app/CN=qd-app" "qd-app,DNS:mimir"
cert worker ca "/O=qd/OU=worker/CN=verify-worker" verify-worker
cert lake ca "/O=qd/OU=lake/CN=verify-lake" verify-lake
cert apps ca "/O=qd/OU=apps/CN=verify-apps" verify-apps
cert cnapp ca "/O=qd/OU=worker/CN=app" app
cert stranger other "/O=qd/OU=app/CN=qd-app" qd-app

MIMIR_FLAGS="-server.http-tls-cert-path=/c/app.pem -server.http-tls-key-path=/c/app.pem -server.http-tls-ca-path=/c/ca.crt -server.http-tls-client-auth=RequireAndVerifyClientCert"
MIMIR_TLS=1 MIMIR_CERT=app
export MIMIR_TLS MIMIR_CERT
mimir_up || exit 1

# --- Traefik with the repository's files ---------------------------------------
DYN=$D/dynamic
mkdir -p "$DYN"
cp "$ROOT/infra/traefik/internal.yml" "$ROOT/infra/traefik/dynamic/middlewares.yml" "$DYN/"
cp "$D/ca.crt" "$DYN/machines-ca.crt" && cp "$D/app.pem" "$DYN/app-machine.pem"
# A control, not a product file: a rule reading the header.
cat >"$DYN/probe.yml" <<'YML'
http:
  routers:
    probe:
      rule: "Host(`qd-app`) && Path(`/probe`) && HeaderRegexp(`X-Forwarded-Tls-Client-Cert-Info`, `OU(%3D|=)app`)"
      entryPoints: [mimir]
      tls:
        options: machines
      middlewares: [mimir-subject]
      service: ping@internal
YML
chmod 755 "$DYN" && chmod 644 "$DYN"/*
docker run -d --name "$P-traefik" --network "$NET" --network-alias qd-app --tmpfs /letsencrypt \
  -e INTERNAL_DOMAIN=qd-app -e MIMIR_UPSTREAM=https://mimir:9009 \
  -e INTERNAL_CERT_FILE=/etc/traefik/dynamic/app-machine.pem -e INTERNAL_KEY_FILE=/etc/traefik/dynamic/app-machine.pem \
  -v "$ROOT/infra/traefik/traefik.yml:/etc/traefik/traefik.yml:ro" -v "$DYN:/etc/traefik/dynamic:ro" \
  "$TRAEFIK_IMAGE" --configFile=/etc/traefik/traefik.yml >/dev/null
at() { # CERT PATH [CURL OPTION...]: status and whether curl failed, through Traefik's `mimir` entry point
  local cert=$1 path=$2 c=(--cacert /c/ca.crt)
  shift 2
  [ -n "$cert" ] && c+=(--cert "/c/$cert.pem")
  docker run --rm --network "$NET" -v "$D:/c:ro" "$CURL" -s -o /dev/null -w '%{http_code}' -m 20 "${c[@]}" "$@" \
    "https://qd-app:9446$path"
  [ $? = 0 ] && echo " ok" || echo " failed"
}
for _ in $(seq 30); do [ "$(at app /ready)" = "200 ok" ] && break; sleep 2; done
Q='/prometheus/api/v1/query?query=up'

echo "== who may read"
expect "the application VM's certificate (OU=app): a query, /metrics and /ready answered" "200 ok 200 ok 200 ok" \
  "$(at app "$Q") $(at app /metrics) $(at app /ready)"
expect "a worker's (OU=worker): refused" "403 ok" "$(at worker "$Q")"
expect "a lake node's (OU=lake): refused" "403 ok" "$(at lake "$Q")"
expect "OU=apps, and OU=worker with CN=app: refused (the unit, whole)" "403 ok 403 ok" "$(at apps "$Q") $(at cnapp "$Q")"
expect "a worker's /metrics and /ready, and Mimir's other APIs: refused" "403 ok 403 ok 403 ok" \
  "$(at worker /metrics) $(at worker /ready) $(at worker /api/v1/user_stats)"
SPOOF=(-H 'X-Forwarded-Tls-Client-Cert-Info: Subject%3D%22OU%3Dapp%22')
expect "a worker sending the header of an app certificate itself: refused" "403 ok" "$(at worker "$Q" "${SPOOF[@]}")"
expect "control: a rule on that header matches neither a client's own header nor the certificate's (Mimir's 404 for app, 403 for the worker)" \
  "404 ok 403 ok" "$(at app /probe) $(at worker /probe "${SPOOF[@]}")"
expect "a path leaving /api/v1/push (curl --path-as-is): refused" "403 ok" \
  "$(at worker /api/v1/push/../../prometheus/api/v1/query?query=up --path-as-is)"
expect "no certificate: no handshake" "000 failed" "$(at "" "$Q")"
expect "another CA's certificate: no handshake" "000 failed" "$(at stranger "$Q")"
expect "the check's entry point is Traefik's loopback alone (refused from another container)" "000 failed" \
  "$(docker run --rm --network "$NET" "$CURL" -s -o /dev/null -w '%{http_code}' -m 5 http://qd-app:9447/ ; [ $? = 0 ] && echo " ok" || echo " failed")"

echo "== who may push"
expect "an empty remote write with a worker's certificate reaches Mimir's handler (its 4xx, not Traefik's 403)" "400 ok" \
  "$(at worker /api/v1/push -X POST -H 'Content-Type: application/x-protobuf' -H 'Content-Encoding: snappy')"
sed -n '/^prometheus.remote_write "mimir"/,/^}/p' "$ROOT/infra/observability/alloy/worker.alloy" | grep -v 'role *=' >"$D/rw.worker"
same=0
for r in lake lake-replica; do
  sed -n '/^prometheus.remote_write "mimir"/,/^}/p' "$ROOT/infra/observability/alloy/$r.alloy" | grep -v 'role *=' | cmp -s - "$D/rw.worker" && same=$((same + 1))
done
expect "the lake roles' remote write block is the worker's but its role label" 2 "$same"
# A copy readable by Alloy's user (the checkout's may be 0640).
mkdir -p "$D/alloy" && install -m 644 "$ROOT/infra/observability/alloy/worker.alloy" "$D/alloy/" && chmod 755 "$D/alloy"
alloy() { # NAME CERT VM: the repository's worker.alloy pushing through Traefik with CERT
  docker run -d --name "$P-alloy-$1" --network "$NET" --user 65532:65532 --read-only --tmpfs /tmp \
    -e OBS_VM="$3" -e METRICS_PUSH_URL=https://qd-app:9446/api/v1/push \
    -e METRICS_CA_FILE=/c/ca.crt -e METRICS_CERT_FILE="/c/$2.pem" \
    -v "$D:/c:ro" -v "$D/alloy:/etc/qd-alloy:ro" \
    "$ALLOY_IMAGE" run --storage.path=/tmp/alloy --disable-reporting /etc/qd-alloy >/dev/null
}
alloy worker worker verify-worker
alloy lake lake verify-lake
alloy app app verify-app
n=none
for _ in $(seq 40); do
  n=$(scalar 'count(count by (vm) (up{job="alloy",vm=~"verify-(worker|lake|app)"}))')
  [ "$n" = 3 ] && break
  sleep 3
done
expect "each Alloy's series in Mimir (worker, lake node, application VM)" 3 "$n"
expect "and none of them failed a sample" 0 \
  "$(scalar 'sum(prometheus_remote_storage_samples_failed_total{vm=~"verify-(worker|lake|app)"})')"

echo "== Grafana's datasource, as grafana/tls/entrypoint.sh provisions it"
mkdir -p "$D/grafana/provisioning/datasources" "$D/agent" "$D/secrets"
cp "$ROOT"/infra/observability/grafana/provisioning/datasources/*.yml "$D/grafana/provisioning/datasources/"
cp "$D/ca.crt" "$D/agent/ca.crt" && cp "$D/app.pem" "$D/agent/machine.pem"
openssl rand -hex 16 >"$D/secrets/grafana_pg_password"
GPW=$(openssl rand -hex 16)
chmod -R a+rX "$D/grafana" "$D/agent" "$D/secrets"
docker run -d --name "$P-grafana" --network "$NET" --user 0:0 --entrypoint /etc/qd/grafana/entrypoint.sh \
  --tmpfs /run/qd-grafana:mode=0755 -e GF_PATHS_PROVISIONING=/run/qd-grafana/provisioning \
  -e GF_SECURITY_ADMIN_PASSWORD="$GPW" -e MIMIR_URL=https://mimir:9009 -e MIMIR_SERVER_NAME=qd-app \
  -e LOKI_URL=https://qd-app:3100 -e GRAFANA_PG_HOST=qd-pg -e GRAFANA_PG_PORT=5432 -e GRAFANA_PG_DATABASE=postgres \
  -e GRAFANA_PG_SSLMODE=verify-full \
  -v "$ROOT/infra/observability/grafana/tls/entrypoint.sh:/etc/qd/grafana/entrypoint.sh:ro" \
  -v "$D/agent:/run/qd-node:ro" -v "$D/grafana/provisioning:/etc/grafana/provisioning:ro" \
  -v "$D/secrets:/run/secrets:ro" "$GRAFANA_IMAGE" >/dev/null
gf() { docker exec "$P-grafana" wget -qO- --header "Authorization: Basic $(printf 'admin:%s' "$GPW" | base64 -w0)" "$@" 2>/dev/null; }
health=none
for _ in $(seq 60); do
  health=$(gf http://127.0.0.1:3000/api/datasources/uid/mimir/health | python3 -I -c 'import json, sys; print(json.load(sys.stdin).get("status"))' 2>/dev/null)
  [ "$health" = OK ] && break
  sleep 2
done
expect "its health check, Mimir by its service name with the application VM's certificate" OK "$health"
q='{"queries":[{"refId":"A","datasource":{"uid":"mimir"},"expr":"up{job=\"alloy\",vm=~\"verify-.*\"}","instant":true}],"from":"now-10m","to":"now"}'
gf --header 'Content-Type: application/json' --post-data "$q" http://127.0.0.1:3000/api/ds/query >"$D/ds.json"
expect "a PromQL query through Grafana's API answers the three machines' series" "200 3" \
  "$(python3 -I -c '
import json, sys
r = json.load(sys.stdin)["results"]["A"]
print(r.get("status"), sum(1 for f in r.get("frames", []) if f.get("data", {}).get("values")))' <"$D/ds.json" 2>/dev/null)"
[ "$FAILED" = 0 ] || note "Grafana's answer: $(head -c 300 "$D/ds.json")"

note "Traefik's errors (a docker provider without a socket and an ACME store are expected): $(docker logs "$P-traefik" 2>&1 | grep -c '"level":"error"') lines; about Mimir: $(docker logs "$P-traefik" 2>&1 | grep -i mimir | grep -c '"level":"error"')"
echo "   $(docker run --rm "$TRAEFIK_IMAGE" version 2>/dev/null | sed -n 's/^Version: *//p' | sed 's/^/Traefik /'), Mimir 3.2.4, Alloy v1.21.3, Grafana 13.2.5, curl 8.11.1"
echo "$PASSED passed, $FAILED failed"
[ $FAILED = 0 ] && echo "ALL OK" || echo "SOME FAILED"
exit $((FAILED > 0))
