#!/bin/bash
# Verification of decision 0076, step 0: the contraction of tenants on the
# package's PostgreSQL (postgres:16-alpine, as crates/testkit) with
# synthetic rows. The core migrations of an API image (QD_API_IMAGE, by
# default the newest local qd-api image: the version the contraction
# follows) applied by the image itself (`server audit verify` runs them
# through sqlx), the quality module's of this checkout in order with psql,
# then up to two synthetic rows in every table (tenant_contract_fill.sql,
# from the catalog) and the audit chain verified.
#
# With the contraction migrations as arguments (core first, then the
# module's), it applies them and checks: the row count of every table that
# stays is unchanged (but audit_chain_checks, which gains a row per
# verification), every table that had a primary key still has one, as
# many foreign keys as before not counting those to `public.tenants` and
# those of `login_handoffs` (both dropped, decision 0076 point 7), no
# `tenant_id` left but the audit chain's (decision 0076, point 6), no
# row-level security left, the chain still verifies, and a second run of the
# contraction changes nothing. Without arguments, only the first part runs.
#
# Only table names, counts and statuses are printed. Containers and the
# network qd-verify-tenantc-…, removed on exit; no port published. Run it
# alone, with no build running.
#
#   QD_API_IMAGE=qd-api:<sha> bash tools/verify/tenant_contract.sh [CONTRACT.sql ...]
P=qd-verify-tenantc
NET_OPTS=--internal
. "$(dirname "$0")/machines_lib.sh"
API_IMAGE=${QD_API_IMAGE:-$(docker images --format '{{.Repository}}:{{.Tag}}' qd-api | head -1)}
[ -n "$API_IMAGE" ] || { echo "no qd-api image: set QD_API_IMAGE"; exit 2; }
for f in "$@"; do [ -f "$f" ] || { echo "no such file: $f"; exit 2; }; done
start_run
note "API image $API_IMAGE"

docker run -d --name "$P-pg" --network "$NET" --network-alias qd-postgres \
  -e POSTGRES_PASSWORD=verify postgres:16-alpine >/dev/null
psql_() { docker exec -i "$P-pg" psql -XAtq -v ON_ERROR_STOP=1 -U postgres -d postgres "$@"; }
for _ in $(seq 60); do psql_ -c 'SELECT 1' >/dev/null 2>&1 && break; sleep 1; done
audit() {
  docker run --rm --network "$NET" -e DATABASE_URL=postgres://postgres:verify@qd-postgres:5432/postgres \
    -e RUST_LOG=error "$API_IMAGE" audit verify --lang en >"$D/audit.out" 2>&1
}
tables() {
  psql_ -c "SELECT n.nspname || '.' || c.relname || ' ' || (xpath('/row/c/text()',
      query_to_xml(format('SELECT count(*) AS c FROM %I.%I', n.nspname, c.relname), false, true, '')))[1]::text
    FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
    WHERE c.relkind IN ('r', 'p') AND n.nspname IN ('public', 'quality') AND NOT c.relispartition
      AND c.relname NOT IN ('_sqlx_migrations', '_module_migrations')" | sort
}
with_key() {
  psql_ -c "SELECT conrelid::regclass::text FROM pg_constraint WHERE contype = 'p'
    AND connamespace::regnamespace::text IN ('public', 'quality')" | sort
}
foreign_keys() {
  psql_ -c "SELECT count(*) FROM pg_constraint WHERE contype = 'f'
    AND connamespace::regnamespace::text IN ('public', 'quality')
    AND confrelid <> coalesce(to_regclass('public.tenants'), 0)
    AND conrelid <> coalesce(to_regclass('public.login_handoffs'), 0)"
}

# 1. The schema as an installation has it: the queue's role first, as
# infra/postgres/initdb creates it.
psql_ -c "CREATE ROLE qd_queue NOLOGIN" >/dev/null
expect "the API image applies the core migrations and verifies an empty chain" yes "$(yn audit)"
applied=0
for f in "$ROOT"/crates/quality/migrations/*.sql; do
  psql_ -f - <"$f" >/dev/null 2>"$D/err" || { ko "quality migration $(basename "$f"): $(head -1 "$D/err")"; break; }
  applied=$((applied + 1))
done
expect "the quality migrations applied" "$(ls "$ROOT"/crates/quality/migrations/*.sql | wc -l)" "$applied"

# 2. Synthetic rows in every table, and the chain over them.
unfilled=$(psql_ -f - <"$ROOT/tools/verify/tenant_contract_fill.sql")
[ -z "$unfilled" ] && ok "every table has synthetic rows" || {
  ko "tables without synthetic rows:"; printf '       %s\n' "$unfilled"; }
tables >"$D/before"
note "$(wc -l <"$D/before") tables, $(awk '{s += $2} END {print s}' "$D/before") rows"
empty=$(awk '$2 == 0 {print $1}' "$D/before" | tr '\n' ' ')
expect "no table left empty" "" "$empty"
expect "the audit chain verifies with the rows" yes "$(yn audit)"
with_key >"$D/keys_before"
fks_before=$(foreign_keys)

# 3. The contraction, twice.
if [ $# -eq 0 ]; then
  note "no contraction migrations given: steps 1 and 2 only"
else
  for run in first second; do
    for f in "$@"; do
      psql_ -f - <"$f" >/dev/null 2>"$D/err" && ok "$run run of $(basename "$f")" ||
        ko "$run run of $(basename "$f"): $(head -1 "$D/err")"
    done
    tables >"$D/after_$run"
    expect "$run run: the tables that stay keep their rows" "" \
      "$(join "$D/before" "$D/after_$run" | awk '$2 != $3 && $1 != "public.audit_chain_checks" {print $1}' | tr '\n' ' ')"
    expect "$run run: the tables dropped" "public.login_handoffs public.tenants " \
      "$(join -v1 "$D/before" "$D/after_$run" | awk '{print $1}' | tr '\n' ' ')"
    expect "$run run: the tables that had a primary key still have one" "public.login_handoffs public.tenants " \
      "$(comm -23 "$D/keys_before" <(with_key) | tr '\n' ' ')"
    expect "$run run: the foreign keys rebuilt" "$fks_before" "$(foreign_keys)"
    expect "$run run: tenant_id only in the audit chain" "admin_audit_events audit_chain_heads " \
      "$(psql_ -c "SELECT DISTINCT table_name FROM information_schema.columns WHERE column_name = 'tenant_id'
         AND table_schema IN ('public', 'quality') ORDER BY 1" | tr '\n' ' ')"
    expect "$run run: no row-level security" 0 \
      "$(psql_ -c "SELECT count(*) FROM pg_class WHERE relrowsecurity OR relforcerowsecurity")"
    expect "$run run: the audit chain verifies" yes "$(yn audit)"
  done
fi

echo "passed $PASSED, failed $FAILED"
[ "$FAILED" -eq 0 ]
