#!/bin/bash
# Verification V7 of decision 0073 (point 5, ruled 2026-11-16): Grafana
# 13.2.5's own database in the platform's PostgreSQL, two Grafana on it.
# PostgreSQL is the repository's: postgres:16-alpine started as
# infra/postgres/tls.yml starts it (its entrypoint.sh, pg_hba.conf and
# pg_ident.conf, TLS with a machine certificate of a throwaway CA, every
# connection without TLS refused), a database `grafana` and a role of its
# own with a password. Grafana as grafana_quality_lib.sh starts it, plus
# GF_DATABASE_* with the password from a file, verify-full against the CA.
#
# It shows: Grafana refused without TLS; both instances migrate and start on
# one database; the organisation, users, sessions, provisioned dashboards
# and an alert rule's state there; a session made through one used on the
# other; both provisioning the same files without conflict; a restart of
# both keeping the alert's state (0072 point 5, which assumed grafana-data).
# Only names, counts and statuses are printed.
#
# Containers qd-verify-gfpg-…, network of that name, removed on exit; no
# port published. Run it alone.
#
#   bash tools/verify/grafana_postgres_db.sh
P=qd-verify-gfpg
NET_OPTS=--internal
. "$(dirname "$0")/machines_lib.sh"
. "$(dirname "$0")/grafana_quality_lib.sh"
start_run
infinity_plugin
quality_files
cert pg ca "/O=qd/OU=app/CN=qd-postgres" qd-postgres
mkdir -p "$D/node" && cp "$D/ca.crt" "$D/node/ca.crt" && cat "$D/pg.crt" "$D/pg.key" >"$D/node/machine.pem"
chmod 755 "$D/node" && chmod 644 "$D/node/"*
openssl rand -hex 16 >"$D/c/pg_super" && openssl rand -hex 16 >"$D/c/grafana_db" && chmod 644 "$D/c/pg_super" "$D/c/grafana_db"
docker run -d --name "$P-pg" --network "$NET" --network-alias qd-postgres --init \
  -v "$ROOT/infra/postgres/tls:/etc/qd/postgres:ro" -v "$D/node:/run/qd-node:ro" -v "$D/c:/c:ro" \
  -e POSTGRES_PASSWORD_FILE=/c/pg_super -e POSTGRES_DB=knime --entrypoint /etc/qd/postgres/entrypoint.sh postgres:16-alpine \
  postgres -c ssl=on -c ssl_cert_file=/var/lib/postgresql/tls/machine.pem -c ssl_key_file=/var/lib/postgresql/tls/machine.pem \
  -c ssl_ca_file=/var/lib/postgresql/tls/ca.crt -c hba_file=/var/lib/postgresql/tls/pg_hba.conf \
  -c ident_file=/var/lib/postgresql/tls/pg_ident.conf >/dev/null
psql_() { docker exec "$P-pg" psql -XAtq -U postgres -d "${2:-knime}" -c "$1"; }
for i in $(seq 60); do [ "$(psql_ 'show ssl' 2>/dev/null)" = on ] && break; sleep 2; done
expect "PostgreSQL with TLS" on "$(psql_ 'show ssl')"
docker exec -i "$P-pg" psql -XAtq -U postgres -d knime -v ON_ERROR_STOP=1 >/dev/null <<SQL
\set pw \`cat /c/grafana_db\`
CREATE ROLE grafana LOGIN PASSWORD :'pw';
CREATE DATABASE grafana OWNER grafana;
SQL
expect "the database grafana, owned by its role" grafana "$(psql_ "SELECT pg_get_userbyid(datdba) FROM pg_database WHERE datname = 'grafana'")"

DB=(-e GF_DATABASE_TYPE=postgres -e GF_DATABASE_HOST=qd-postgres:5432 -e GF_DATABASE_NAME=grafana
  -e GF_DATABASE_USER=grafana -e GF_DATABASE_PASSWORD__FILE=/c/grafana_db -e GF_DATABASE_SSL_MODE=verify-full
  -e GF_DATABASE_CA_CERT_PATH=/c/ca.crt -e GF_DATABASE_SERVER_CERT_NAME=qd-postgres)
# An alert rule from files (expressions only, no data source), firing.
mkdir -p "$D/prov/alerting"
cat >"$D/prov/alerting/rules.yml" <<'YML'
apiVersion: 1
groups:
  - orgId: 1
    name: verify
    folder: qd-verify
    interval: 10s
    rules:
      - uid: qd-verify-always
        title: always
        condition: B
        for: 0s
        noDataState: OK
        execErrState: Error
        data:
          - { refId: A, datasourceUid: __expr__, model: { refId: A, type: math, expression: "1" } }
          - { refId: B, datasourceUid: __expr__, model: { refId: B, type: threshold, expression: A, conditions: [ { evaluator: { type: gt, params: [ 0 ] } } ] } }
YML
mkdir -p "$D/dash/audit" && printf '{"uid":"qd-audit-en","title":"Audit trail","schemaVersion":42,"panels":[]}' >"$D/dash/audit/a.json"
printf 'apiVersion: 1\nproviders:\n  - { name: audit, orgId: 1, type: file, disableDeletion: true, options: { path: /dash/audit } }\n' >"$D/prov/dashboards/audit.yml"
chmod -R a+rX "$D/prov" "$D/dash"

echo "--- without TLS"
quality_grafana_up grafana "${DB[@]}" -e GF_DATABASE_SSL_MODE=disable >/dev/null 2>&1
expect "Grafana with sslmode disable keeps running" no "$(docker ps -q --filter "name=^/$P-grafana\$" | grep -q . && echo yes || echo no)"
expect "PostgreSQL said why" yes "$(docker logs "$P-pg" 2>&1 | grep -q 'pg_hba.conf rejects connection.*user "grafana".*no encryption' && echo yes || echo no)"
docker rm -f "$P-grafana" >/dev/null

echo "--- two Grafana on one database"
quality_edge_up
quality_grafana_up grafana "${DB[@]}" || exit 1
ok "Grafana A migrated and started, verify-full"
quality_grafana_up grafana-b "${DB[@]}" || exit 1
ok "Grafana B started on the same database"
note "Grafana's tables in the database: $(psql_ "SELECT count(*) FROM pg_tables WHERE tableowner = 'grafana'" grafana)"
expect "Quality made through A, seen by B" 2 "$(gfa grafana /api/orgs -X POST -H 'Content-Type: application/json' -d '{"name":"Quality"}' >/dev/null
  gfa grafana-b /api/orgs | python3 -I -c 'import json,sys; print(*[o["id"] for o in json.load(sys.stdin) if o["name"] == "Quality"])')"
# 13.2.5 keeps dashboards in its unified storage (table resource), not in
# the table dashboard, which stays empty.
expect "the provisioned dashboard once, read by both" "1 qd-audit-en qd-audit-en" \
  "$(echo "$(psql_ "SELECT count(*) FROM resource WHERE \"group\" = 'dashboard.grafana.app' AND name = 'qd-audit-en'" grafana)" \
  "$(gfa grafana '/api/search?type=dash-db' | python3 -I -c 'import json,sys; print(*[d["uid"] for d in json.load(sys.stdin)])')" \
  "$(gfa grafana-b '/api/search?type=dash-db' | python3 -I -c 'import json,sys; print(*[d["uid"] for d in json.load(sys.stdin)])')")"
expect "the provisioned alert rule once" 1 "$(psql_ "SELECT count(*) FROM alert_rule WHERE uid = 'qd-verify-always'" grafana)"
note "provisioning errors in either log: $(docker logs "$P-grafana" 2>&1 | grep -ci 'provision.*error'), $(docker logs "$P-grafana-b" 2>&1 | grep -ci 'provision.*error')"

echo "--- a session made through A, used on B"
expect "ana signs in through A (Traefik's grafana)" "200 https://$APP/grafana/" "$(signin ana)"
expect "her session row in the database" 1 "$(psql_ "SELECT count(*) FROM user_auth_token t JOIN \"user\" u ON u.id = t.user_id WHERE u.login = 'ana@verify.example'" grafana)"
# The cookie read inside the container and sent to B over plain HTTP; never printed.
expect "the same session on B" "200 ana@verify.example" "$(docker run --rm --network "$NET" -v "$D/jar:/jar:ro" --entrypoint sh "$CURL" -c \
  's=$(awk '"'"'$6 == "grafana_session" {print $7}'"'"' /jar/ana); curl -s -m 20 -H "Cookie: grafana_session=$s" -w "\n%{http_code}" http://grafana-b:3000/grafana/api/user' |
  python3 -I -c 'import json,sys; b, c = sys.stdin.read().rsplit("\n", 1); print(c, json.loads(b).get("login") if c == "200" else "")')"

echo "--- the alert's state, and a restart of both"
st() { gfa "$1" /api/prometheus/grafana/api/v1/rules | python3 -I -c 'import json,sys
d = json.load(sys.stdin)
print(*[r["state"] + " " + (r.get("alerts") or [{}])[0].get("activeAt", "-") for g in d["data"]["groups"] for r in g["rules"] if r["name"] == "always"] or ["none"])'; }
for i in $(seq 30); do case "$(st grafana)" in firing*) break ;; esac; sleep 2; done
A=$(st grafana)
expect "the rule firing, seen through A" firing "${A%% *}"
expect "and through B, active since the same moment" "$A" "$(st grafana-b)"
# 13.2.5 keeps the alerts' state in alert_rule_state (alert_instance stays empty).
note "rows of alert_rule_state for the rule: $(psql_ "SELECT count(*) FROM alert_rule_state WHERE rule_uid = 'qd-verify-always'" grafana), of alert_instance: $(psql_ "SELECT count(*) FROM alert_instance" grafana)"
docker restart "$P-grafana" "$P-grafana-b" >/dev/null
for i in $(seq 60); do [ "$(gfa grafana /api/health -o /dev/null -w '%{http_code}')" = 200 ] && break; sleep 2; done
sleep 25
expect "after both restarted, still firing since the same moment, on A" "$A" "$(st grafana)"
expect "and on B" "$A" "$(st grafana-b)"
expect "ana's session still good after the restarts" 200 "$(as ana /api/user -o /dev/null -w '%{http_code}')"

echo
echo "passed $PASSED, failed $FAILED"
[ "$FAILED" = 0 ] && echo "ALL OK" || exit 1
