#!/bin/bash
# Verification for decision 0090, T3: bronze loaded append-only on RustFS
# 1.0.5 with Trino 484. A bronze table as today's (partitioned by cut, rows
# already in it); ALTER TABLE ADD COLUMN _load; partition evolution from
# (cut) to (cut, _load) through RustFS's REST catalog; an extraction's
# INSERT … SELECT as one commit; the loads listed in $partitions; a
# partition-only DELETE of a superseded load (metadata only: no delete
# files); the old rows read with a NULL _load; reads pinned FOR VERSION AS
# OF a recorded snapshot, before and after later loads and the delete.
#
# One throwaway RustFS and one Trino (2 GB heap) on a network of their
# own, no port published; rows from Trino's tpch connector (synthetic);
# everything removed on exit. Run from the repository root:
#
#   bash tools/verify/trino_bronze_loads.sh
#
# Needs docker.
set -u
export LC_ALL=C
cd "$(dirname "$0")/../.."
P=qd-verify-t3
. tools/verify/trino_engine_lib.sh
te_rustfs || exit 1
te_trino || exit 1
U=qd-quality
T=lake.loans_bronze.accounts
OLD="DATE '2026-10-31'"
NEW="DATE '2026-11-30'"
q() { tq "$U" "$1"; }
snapshots() { q "SELECT count(*) FROM lake.loans_bronze.\"accounts\$snapshots\""; }
current() { q "SELECT snapshot_id FROM lake.loans_bronze.\"accounts\$refs\" WHERE name = 'main'"; }
delete_files() { q "SELECT count(*) FROM lake.loans_bronze.\"accounts\$files\" WHERE content <> 0"; }
last_summary() {
  q "SELECT operation || ' deleted-data-files=' || coalesce(element_at(summary, 'deleted-data-files'), '-')
    || ' added-delete-files=' || coalesce(element_at(summary, 'added-delete-files'), '-')
    || ' added-data-files=' || coalesce(element_at(summary, 'added-data-files'), '-')
    FROM lake.loans_bronze.\"accounts\$snapshots\" ORDER BY committed_at DESC LIMIT 1"
}
tq_ok "version" "$U" "SELECT node_version FROM system.runtime.nodes"

echo "== a bronze table as today's, partitioned by cut"
tq_ok "schema" "$U" "CREATE SCHEMA lake.loans_bronze"
tq_ok "table by cut" "$U" "CREATE TABLE $T (id bigint, name varchar, balance double, cut date)
  WITH (partitioning = ARRAY['cut'])"
tq_ok "the old cut's rows" "$U" "INSERT INTO $T SELECT custkey, name, acctbal, $OLD FROM tpch.tiny.customer"

echo "== ADD COLUMN _load and partition evolution"
tq_ok "ALTER TABLE ADD COLUMN _load" "$U" "ALTER TABLE $T ADD COLUMN _load varchar"
expect "old rows read with a NULL _load" "1500 1500" \
  "$(q "SELECT CAST(count(*) AS varchar) || ' ' || CAST(count_if(_load IS NULL) AS varchar) FROM $T WHERE cut = $OLD")"
tq_ok "SET PROPERTIES partitioning = (cut, _load)" "$U" \
  "ALTER TABLE $T SET PROPERTIES partitioning = ARRAY['cut', '_load']"
expect_match "the table's partitioning now" "cut.*_load" \
  "$(q "SHOW CREATE TABLE $T" | grep -i partitioning)"

echo "== an extraction: one INSERT, one commit"
before=$(snapshots)
tq_ok "INSERT … SELECT of load job-a (150 000 rows)" "$U" \
  "INSERT INTO $T SELECT custkey, name, acctbal, $NEW, 'job-a' FROM tpch.sf1.customer"
expect "snapshots added by it" "1" "$(($(snapshots) - before))"
info "data files it wrote: $(q "SELECT count(*) FROM lake.loans_bronze.\"accounts\$files\" WHERE partition.\"_load\" = 'job-a'")"
ACCEPTED=$(current)
expect_match "the accepted load's snapshot from \$refs" '^[0-9-]+$' "$ACCEPTED"
tq_ok "a retry: load job-b of the same cut" "$U" \
  "INSERT INTO $T SELECT custkey, name, acctbal, $NEW, 'job-b' FROM tpch.sf1.customer WHERE custkey <= 1000"
expect "the loads of the cut listed in \$partitions" "job-a job-b" \
  "$(q "SELECT array_join(array_agg(partition.\"_load\" ORDER BY partition.\"_load\"), ' ')
       FROM lake.loans_bronze.\"accounts\$partitions\" WHERE partition.cut = $NEW")"
expect "a reader pinned to the accepted load (job-a)" "150000" \
  "$(q "SELECT count(*) FROM $T FOR VERSION AS OF $ACCEPTED WHERE cut = $NEW AND _load = 'job-a'")"
expect "the same reader unpinned sees both loads" "151000" "$(q "SELECT count(*) FROM $T WHERE cut = $NEW")"

echo "== a superseded load dropped by a partition-only DELETE"
tq_ok "DELETE … WHERE cut = ? AND _load = 'job-b'" "$U" "DELETE FROM $T WHERE cut = $NEW AND _load = 'job-b'"
expect_match "its snapshot: whole files removed, no delete files" \
  "^delete deleted-data-files=[1-9][0-9]* added-delete-files=- " "$(last_summary)"
expect "delete files in the table" "0" "$(delete_files)"
expect "job-a untouched" "150000" "$(q "SELECT count(*) FROM $T WHERE cut = $NEW")"
expect "the pinned read after the delete" "150000" \
  "$(q "SELECT count(*) FROM $T FOR VERSION AS OF $ACCEPTED WHERE cut = $NEW AND _load = 'job-a'")"
out=$(tq "$U" "DELETE FROM $T WHERE cut = $NEW AND _load = 'job-b'")
if [ $? -ne 0 ]; then info "the same DELETE again (no rows): refused: $(printf '%s' "$out" | head -c 160)"; else
  info "the same DELETE again (no rows): accepted: $out"; fi

echo "== the old cut (old partition spec, _load NULL)"
expect "old rows still read" "1500" "$(q "SELECT count(*) FROM $T WHERE cut = $OLD AND _load IS NULL")"
tq_ok "an old cut replaced by a new load (job-c)" "$U" \
  "INSERT INTO $T SELECT custkey, name, acctbal, $OLD, 'job-c' FROM tpch.tiny.customer"
tq_ok "DELETE of the old cut's NULL-load rows" "$U" "DELETE FROM $T WHERE cut = $OLD AND _load IS NULL"
info "  its snapshot: $(last_summary); delete files now: $(delete_files)"
expect "the old cut now holds job-c only" "1500 job-c" \
  "$(q "SELECT CAST(count(*) AS varchar) || ' ' || array_join(array_agg(DISTINCT _load), ',') FROM $T WHERE cut = $OLD")"

echo "== control: a table partitioned by (cut, _load) from its creation"
tq_ok "the control table" "$U" "CREATE TABLE lake.loans_bronze.control WITH (partitioning = ARRAY['cut', '_load'])
  AS SELECT * FROM $T WHERE cut = $NEW"
tq_ok "  a second load" "$U" "INSERT INTO lake.loans_bronze.control
  SELECT id, name, balance, cut, 'job-b' FROM $T WHERE cut = $NEW AND id <= 1000"
tq_ok "  DELETE of that load" "$U" "DELETE FROM lake.loans_bronze.control WHERE cut = $NEW AND _load = 'job-b'"
expect "  delete files in the control table" "0" \
  "$(q "SELECT count(*) FROM lake.loans_bronze.\"control\$files\" WHERE content <> 0")"

echo "== the evolved table after optimize"
tq_ok "optimize (rewrites the old spec's files)" "$U" "ALTER TABLE $T EXECUTE optimize"
info "files by partition spec after it: $(q "SELECT CAST(spec_id AS varchar) || ':' || CAST(count(*) AS varchar)
  FROM lake.loans_bronze.\"accounts\$files\" GROUP BY spec_id ORDER BY spec_id" | paste -sd' ')"
tq_ok "a new load (job-d)" "$U" "INSERT INTO $T SELECT custkey, name, acctbal, $NEW, 'job-d' FROM tpch.tiny.customer"
tq_ok "DELETE of job-d" "$U" "DELETE FROM $T WHERE cut = $NEW AND _load = 'job-d'"
info "  its snapshot: $(last_summary)"
[ "$FAILED" = 0 ] && echo "ALL OK" || echo "SOME CHECKS FAILED"
exit "$FAILED"
