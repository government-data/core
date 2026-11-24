#!/bin/bash
# Verification for decision 0090, T20: an extraction read by key ranges.
# A SQL Server 2019 table of 2 000 000 synthetic rows; one INSERT into
# Iceberg bronze whose SELECT is N UNION ALL range scans on a numeric key:
# EXPLAIN shows N table scans, each with its range pushed to the source;
# how many reader sessions the source runs at once during it; one snapshot;
# its time against one stream reading the whole table; a range on a text
# key, which must not be pushed.
#
# One SQL Server (TLS, the read-only login of trino_source_lib.sh), one
# RustFS and one Trino (2 GB heap, dynamic catalogs in memory) on a network
# of their own, no port published; synthetic rows; everything removed on
# exit. Run from the repository root:
#
#   bash tools/verify/trino_extract_ranges.sh [N]
#
# Needs docker and openssl.
set -u
export LC_ALL=C
cd "$(dirname "$0")/../.."
P=qd-verify-t20
. tools/verify/trino_engine_lib.sh
. tools/verify/trino_source_lib.sh
N=${1:-4}
ROWS=2000000
V=2019
te_rustfs || exit 1
TRINO_CONFIG=$DYNAMIC te_trino || exit 1
out=$(te_dynamic_lake) || { ko "the lake catalog: $out"; exit 1; }
ms_start "$V" || exit 1
ms_reader "$V" || exit 1
ms_truststore "$V"
ms "$V" "USE src; CREATE TABLE ops.loans (id bigint NOT NULL PRIMARY KEY, code varchar(12) NOT NULL,
  amount decimal(12, 2) NOT NULL, opened date NOT NULL);
  WITH n AS (SELECT TOP ($ROWS) ROW_NUMBER() OVER (ORDER BY (SELECT NULL)) AS i
    FROM sys.all_objects a CROSS JOIN sys.all_objects b CROSS JOIN sys.all_objects c)
  INSERT INTO ops.loans SELECT i, CONCAT(CHAR(65 + i % 26), RIGHT(CONCAT('0000000000', i), 10)),
    (i % 100000) / 7.0, DATEADD(day, CAST(i % 3650 AS int), '2016-01-01') FROM n;
  CREATE INDEX ix_code ON ops.loans (code)" >/dev/null || { ko "the source table"; exit 1; }
ms_catalog src "$V" "$(tls_store "$V")" >/dev/null
U=qd-quality
expect "source rows seen through Trino" "$ROWS" "$(tq $U "SELECT count(*) FROM src.ops.loans")"
tq_ok "bronze schema" $U "CREATE SCHEMA lake.src_bronze"
for t in ranged single; do
  tq $U "CREATE TABLE lake.src_bronze.$t (id bigint, code varchar, amount decimal(12, 2), opened date,
    _load varchar)" >/dev/null
done

SEL="CAST(id AS bigint), CAST(code AS varchar), CAST(amount AS decimal(12, 2)), CAST(opened AS date)"
step=$((ROWS / N)) branches=""
for i in $(seq 0 $((N - 1))); do
  lo=$((i * step + 1)) hi=$(((i + 1) * step + 1))
  [ "$i" = $((N - 1)) ] && hi=$((ROWS + 1))
  branches="$branches${branches:+ UNION ALL }SELECT $SEL, 'job-r' FROM src.ops.loans WHERE id >= $lo AND id < $hi"
done
RANGED="INSERT INTO lake.src_bronze.ranged $branches"

echo "== the plan"
tq $U "EXPLAIN $RANGED" | sed 's/\\n/\n/g' >"$D/plan"
# A scan is TableScan, ScanProject or ScanFilter[Project] over the source table.
expect "scans of the source table in the plan" "$N" "$(grep -c 'table = src:ops.loans' "$D/plan")"
expect "  each with its range pushed (constraint on [id])" "$N" \
  "$(grep 'table = src:ops.loans' "$D/plan" | grep -c 'constraint on \[id\]')"
expect "range filters left in Trino (ScanFilter or Filter on id)" "0" \
  "$(grep -E 'ScanFilter|Filter\[' "$D/plan" | grep -c 'id')"

echo "== the ranged INSERT: source sessions, snapshots, time"
sessions() {
  ms "$V" "SELECT count(*) FROM sys.dm_exec_requests r JOIN sys.dm_exec_sessions s ON r.session_id = s.session_id
    WHERE s.login_name = 'reader'" 2>/dev/null | head -1 | tr -d ' \r'
}
( while :; do sessions; sleep 0.5; done >"$D/sessions" ) &
POLL=$!
before=$(tq $U "SELECT count(*) FROM lake.src_bronze.\"ranged\$snapshots\"")
start=$(date +%s.%N)
tq_ok "the ranged INSERT ($N branches)" $U "$RANGED"
t_ranged=$(echo "$(date +%s.%N) - $start" | bc)
kill "$POLL" 2>/dev/null; wait "$POLL" 2>/dev/null
peak=$(sort -n "$D/sessions" | tail -1)
expect "reader requests running at once in the source (peak)" "$N" "${peak:-0}"
info "  samples of running reader requests: $(sort -n "$D/sessions" | uniq -c | tr -s ' ' | paste -sd,)"
expect "snapshots the ranged INSERT added" "1" \
  "$(($(tq $U "SELECT count(*) FROM lake.src_bronze.\"ranged\$snapshots\"") - before))"
expect "rows written" "$ROWS" "$(tq $U "SELECT count(*) FROM lake.src_bronze.ranged")"
start=$(date +%s.%N)
tq_ok "one stream" $U "INSERT INTO lake.src_bronze.single SELECT $SEL, 'job-s' FROM src.ops.loans"
t_single=$(echo "$(date +%s.%N) - $start" | bc)
info "time: $N ranges ${t_ranged}s, one stream ${t_single}s"

echo "== a range on a text key"
tq $U "EXPLAIN SELECT count(*) FROM src.ops.loans WHERE code >= 'C' AND code < 'M'" | sed 's/\\n/\n/g' >"$D/plan-text"
expect "the text range is filtered in Trino, not pushed" "1" \
  "$(grep -E 'ScanFilter|Filter\[' "$D/plan-text" | grep -c 'code')"
grep -E 'TableScan\[|Filter\[' "$D/plan-text" | cut -c1-240 | while read -r l; do info "  $l"; done
[ "$FAILED" = 0 ] && echo "ALL OK" || echo "SOME CHECKS FAILED"
exit "$FAILED"
