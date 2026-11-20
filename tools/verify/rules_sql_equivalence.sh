#!/bin/sh
# Equivalence of compiled rules on Trino (decision 0019: what is compiled for
# two engines has an equivalence test). The cases of
# crates/quality-rules-sql/tests/cases run on DuckDB in `cargo test`; this
# runs the same cases, written for Trino, on the dev cluster and compares
# each row's results. Synthetic values only; no table is read. From the
# repository root, with the cluster up (`task trino:up`):
#
#   sh tools/verify/rules_sql_equivalence.sh
#
# Needs cargo, curl and jq; queries run with the dev machine client's token
# (`qd-worker-dev`, QD_WORKER_DEV_CLIENT_SECRET), as trino_cluster_check.sh.
set -u
export LC_ALL=C
cd "$(dirname "$0")/../.."
set -a; . ./.env; set +a
COORD=${TRINO_COORDINATOR:-data-quality-trino-coordinator-1}
ISS=${OIDC_ISSUER:-https://${APP_DOMAIN:-localhost}/api}
TOKEN=$(curl -s --cacert infra/traefik/certs/dev-ca.crt -u "qd-worker-dev:${QD_WORKER_DEV_CLIENT_SECRET:?QD_WORKER_DEV_CLIENT_SECRET is needed}" \
  -d grant_type=client_credentials -d audience=trino "$ISS/oauth/token" | jq -r '.access_token // empty')
[ -n "$TOKEN" ] || { echo "no token for qd-worker-dev"; exit 1; }

POLL='r=$(curl -s --cacert /etc/trino/extra-ca.crt -H "Authorization: Bearer $TOKEN" --data-binary "$Q" https://localhost:8490/v1/statement)
while :; do
  printf "%s\n" "$r"
  [[ $r =~ \"nextUri\":\"([^\"]+)\" ]] || break
  sleep 0.1
  r=$(curl -s --cacert /etc/trino/extra-ca.crt -H "Authorization: Bearer $TOKEN" "${BASH_REMATCH[1]}")
done'

CASES=$(mktemp)
trap 'rm -f "$CASES"' EXIT
cargo run -q -p quality-rules-sql --example cases >"$CASES" || exit 1
FAILED=0
while IFS="$(printf '\t')" read -r name statement expected; do
  got=$(docker exec -e TOKEN="$TOKEN" -e Q="$statement" "$COORD" bash -c "$POLL" |
    jq -sRc '[split("\n")[] | fromjson? | (.data // empty)[]] as $rows
      | ([split("\n")[] | fromjson? | .error.message // empty] | first) as $err
      | if $err then "ERROR: " + $err else $rows end')
  if [ "$(printf '%s' "$got" | jq -c .)" = "$(printf '%s' "$expected" | jq -c .)" ]; then
    echo "[OK  ] $name"
  else
    echo "[FAIL] $name: expected $expected, got $got"
    FAILED=1
  fi
done <"$CASES"
[ "$FAILED" = 0 ] && echo "All cases match on Trino." || echo "Some cases differ."
exit "$FAILED"
