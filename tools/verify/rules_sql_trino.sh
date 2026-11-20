#!/bin/sh
# Verification for decision 0033, point 6, on Trino: how literals are read,
# that a query is one statement, and which regular expression engine runs
# (RE2J: linear in time, no backreferences or lookaround, as DuckDB's RE2).
# Synthetic values only; no table is read. Against the dev stack with the
# cluster up (`task trino:up`), from the repository root:
#
#   sh tools/verify/rules_sql_trino.sh
#
# Needs curl and jq. Queries run with the dev machine client's token
# (`qd-worker-dev`, QD_WORKER_DEV_CLIENT_SECRET), as trino_cluster_check.sh.
set -u
export LC_ALL=C
cd "$(dirname "$0")/../.."
set -a; . ./.env; set +a
COORD=${TRINO_COORDINATOR:-data-quality-trino-coordinator-1}
ISS=${OIDC_ISSUER:-https://${APP_DOMAIN:-localhost}/api}
FAILED=0
TOKEN=$(curl -s --cacert infra/traefik/certs/dev-ca.crt -u "qd-worker-dev:${QD_WORKER_DEV_CLIENT_SECRET:?QD_WORKER_DEV_CLIENT_SECRET is needed}" \
  -d grant_type=client_credentials -d audience=trino "$ISS/oauth/token" | jq -r '.access_token // empty')
[ -n "$TOKEN" ] || { echo "no token for qd-worker-dev"; exit 1; }

# The REST protocol from inside the coordinator (its ports are not
# published), followed until the last page; 20 s at most per query.
POLL='r=$(curl -s --cacert /etc/trino/extra-ca.crt -H "Authorization: Bearer $TOKEN" \
  -H "X-Trino-Session: query_max_run_time=20s" --data-binary "$Q" https://localhost:8490/v1/statement)
while :; do
  printf "%s\n" "$r"
  [[ $r =~ \"nextUri\":\"([^\"]+)\" ]] || break
  sleep 0.1
  r=$(curl -s --cacert /etc/trino/extra-ca.crt -H "Authorization: Bearer $TOKEN" "${BASH_REMATCH[1]}")
done'
q() {
  docker exec -e TOKEN="$TOKEN" -e Q="$1" "$COORD" bash -c "$POLL" |
    jq -rR '(fromjson? // {}) | (.data // [] | .[] | map(tostring) | join("\t")),
      (.error.message // empty | "ERROR: " + .)'
}
# expect NAME SQL PATTERN [MAX_SECONDS]: the answer must match the extended
# regex PATTERN, within MAX_SECONDS if given.
expect() {
  start=$(date +%s.%N)
  out=$(q "$2" | head -c 300 | tr '\n' ' ')
  secs=$(echo "$(date +%s.%N) - $start" | bc)
  if printf '%s' "$out" | grep -Eq "$3" && [ "$(echo "$secs <= ${4:-1000}" | bc)" = 1 ]; then
    s="OK  "
  else
    s="FAIL"
    FAILED=1
  fi
  printf '[%s] %s: %s (%.2fs)\n' "$s" "$1" "$out" "$secs"
}

echo "== Trino $(q 'SELECT version()') (regex-library: $(docker exec "$COORD" sh -c 'cat /etc/trino/config.properties' | sed -n 's/^regex-library=//p'))"
expect "literal: a doubled quote is one quote" "SELECT 'it''s' = concat('it', chr(39), 's')" '^true'
expect "literal: a backslash is not an escape" "SELECT length('a\b'), 'a\b' = concat('a', chr(92), 'b')" '^3	true'
expect "literal: Unicode quotes are plain characters" \
  "SELECT length('‘x’'), '’) OR 1=1 --' = concat(chr(8217), ') OR 1=1 --')" '^3	true'
expect "literal: a comment inside a literal stays text" "SELECT length('-- x /* y */')" '^12'
expect "one statement: a second one is refused" "SELECT 1; SELECT 2" '^ERROR'
expect "regex: anchored match" \
  "SELECT regexp_like('3001234567', '^(?:3\d{9})\$'), regexp_like('x3001234567', '^(?:3\d{9})\$')" '^true	false'
expect "regex: backreference refused (RE2J)" "SELECT regexp_like('aa', '(a)\1')" '^ERROR'
expect "regex: lookahead refused (RE2J)" "SELECT regexp_like('a', '(?=a)a')" '^ERROR'
expect "regex: (a+)+ on 28 a's and a b is linear" \
  "SELECT regexp_like(concat(rpad('', 28, 'a'), 'b'), '^(?:(a+)+)\$')" '^false' 2

[ "$FAILED" = 0 ] && echo "All checks passed." || echo "Some checks failed."
exit "$FAILED"
