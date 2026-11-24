# shellcheck shell=bash
# The concurrency part of tools/verify/trino_gold_replace.sh (decision
# 0090, T2), sourced by it: eight inserts from two datasets into one gold
# table, serialized by a per-table lock as the operator does (point 4: all
# must commit), then without it and with more commit retries (findings,
# reported, not checked): how many commits conflict and how many data files
# the failed commits leave in the lake (orphans: written but named by no
# snapshot). Only inserts touch these tables, so every file a snapshot
# names is in "$files".

# new_table NAME [WITH-properties]: a gold-shaped table for one burst.
new_table() {
  tq qd-quality "CREATE TABLE $G.$1 (dataset varchar, cut date, run_id varchar, rule varchar, failed bigint)
    WITH (partitioning = ARRAY['dataset', 'cut', 'run_id']${2:+, $2})" >/dev/null
}
# orphans TABLE: parquet objects under the table's data/ minus the files
# its snapshots name.
orphans() {
  local path prefix listed live
  path=$(tq qd-quality "SELECT \"\$path\" FROM $G.$1 LIMIT 1")
  prefix=${path#s3://"$WAREHOUSE"/}
  prefix=${prefix%%/data/*}/data/
  te_lake_call GET "/$WAREHOUSE?list-type=2&max-keys=1000&prefix=$prefix" "" admin >/dev/null
  listed=$(grep -o '<Key>[^<]*\.parquet</Key>' "$D/lake.out" | wc -l)
  live=$(tq qd-quality "SELECT count(*) FROM $G.\"$1\$files\"")
  echo $((listed - live))
}
# burst TABLE serial|parallel: four inserts per dataset at once; serial
# takes one lock per table around each insert, as the operator's lock does.
# Prints "<failed> failed, <rows> rows, <orphans> orphans".
burst() {
  local i d n
  rm -f "$D"/burst-*
  for i in 1 2 3 4; do
    for d in loans savings; do
      if [ "$2" = serial ]; then
        flock "$D/lock-$1" bash -c "$(declare -f tq_http); P=$P; tq_http job_rules_$d \
          \"INSERT INTO $G.$1 VALUES ('$d', DATE '2026-11-02', 'c$i', 'r1', $i)\"" >"$D/burst-$d-$i" 2>&1 &
      else
        tq_http "job_rules_$d" "INSERT INTO $G.$1 VALUES ('$d', DATE '2026-11-02', 'c$i', 'r1', $i)" \
          >"$D/burst-$d-$i" 2>&1 &
      fi
    done
  done
  wait
  n=$(grep -l "failed" "$D"/burst-* 2>/dev/null | wc -l)
  grep -h "failed" "$D"/burst-* 2>/dev/null | head -1 | while read -r l; do info "    $l" >&2; done
  echo "$n failed, $(tq qd-quality "SELECT count(*) FROM $G.$1") rows, $(orphans "$1") orphans"
}

echo "== one writer per gold table (the ruled lock): eight inserts, serialized per table"
new_table serialized
expect "serialized inserts all committed, no orphan files" "0 failed, 8 rows, 0 orphans" "$(burst serialized serial)"

echo "== finding, not pass/fail: unserialized inserts with the default commit retries"
new_table unserialized
info "  eight concurrent inserts without the lock: $(burst unserialized parallel)"

echo "== more commit retries: max_commit_retry = 20 (commit.retry.num-retries), unserialized"
for r in 1 2 3; do
  new_table "retry20_$r" "max_commit_retry = 20"
  [ "$r" = 1 ] && info "  the table property set: $(tq qd-quality "SHOW CREATE TABLE $G.retry20_1" |
    grep -o 'max_commit_retry = [0-9]*')"
  info "  run $r of 3, eight concurrent inserts: $(burst "retry20_$r" parallel)"
done
