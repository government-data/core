#!/bin/bash
# Verification for decision 0090, T2: replacing a (dataset, cut) in gold
# on RustFS 1.0.5 with Trino 484. Tables partitioned by dataset, cut and
# run_id (run_summary by dataset, with run_seq); insert new, flip, delete
# old, with a crash between each step and a re-run; DELETE … run_id <> ?
# dropping whole partitions (no delete files in $files); an empty DELETE;
# SECURITY INVOKER views over the READ_ONLY catalog `gold` keeping a
# person's row filter; FOR VERSION AS OF a $refs snapshot; two datasets
# inserting into one table at once; CREATE BRANCH; DROP TABLE (406).
#
# Users (file-based access control): qd-quality, the module's own user,
# makes the tables and views; job_rules_loans and job_rules_savings write
# gold through `lake` with filter dataset = '<d>'; person_loans reads
# `gold` with the same filter. One throwaway RustFS and one Trino (2 GB
# heap) on a network of their own, no port published; synthetic rows;
# everything removed on exit. Run from the repository root:
#
#   bash tools/verify/trino_gold_replace.sh
#
# Needs docker and jq.
set -u
export LC_ALL=C
cd "$(dirname "$0")/../.."
P=qd-verify-t2
. tools/verify/trino_engine_lib.sh
G=lake.quality_gold
CUT="DATE '2026-11-01'"

job() { # DATASET: the job user's table rule on lake.quality_gold
  jq -n --arg d "$1" '{user: ("job_rules_" + $d), catalog: "lake", schema: "quality_gold",
    privileges: ["SELECT", "INSERT", "DELETE"], filter: ("dataset = '\''" + $d + "'\''")}'
}
jq -n --argjson l "$(job loans)" --argjson s "$(job savings)" '{
  catalogs: [{user: "qd-quality", allow: "all"}, {user: "job_rules_.*", catalog: "lake", allow: "all"},
             {user: "person_.*", catalog: "gold", allow: "read-only"}, {allow: "none"}],
  schemas: [{user: "qd-quality", owner: true}, {owner: false}],
  tables: [{user: "qd-quality", privileges: ["SELECT", "INSERT", "DELETE", "UPDATE", "OWNERSHIP"]},
           {user: "job_rules_.*", schema: "quality_gold", table: ".*[$]refs", privileges: ["SELECT"]}, $l, $s,
           {user: "person_loans", catalog: "gold", schema: "quality_gold", privileges: ["SELECT"],
            filter: "dataset = '\''loans'\''"}]}' >"$D/rules.json"
te_rustfs || exit 1
te_trino || exit 1
tq_ok "version" qd-quality "SELECT node_version FROM system.runtime.nodes"

echo "== the module's tables and views (qd-quality)"
tq_ok "schema" qd-quality "CREATE SCHEMA $G"
tq_ok "run_summary by dataset" qd-quality "CREATE TABLE $G.run_summary (dataset varchar, cut date,
  run_id varchar, run_seq bigint, rules bigint) WITH (partitioning = ARRAY['dataset'])"
tq_ok "rule_results by dataset, cut, run_id" qd-quality "CREATE TABLE $G.rule_results (dataset varchar,
  cut date, run_id varchar, rule varchar, failed bigint) WITH (partitioning = ARRAY['dataset', 'cut', 'run_id'])"
tq_ok "view current_run, SECURITY INVOKER" qd-quality "CREATE VIEW $G.current_run SECURITY INVOKER AS
  SELECT dataset, cut, max_by(run_id, run_seq) AS run_id FROM gold.quality_gold.run_summary GROUP BY dataset, cut"
tq_ok "view v_rule_results, SECURITY INVOKER, JOIN … ON" qd-quality "CREATE VIEW $G.v_rule_results SECURITY INVOKER AS
  SELECT t.* FROM gold.quality_gold.rule_results t JOIN gold.quality_gold.current_run r
  ON t.dataset = r.dataset AND t.cut = r.cut AND t.run_id = r.run_id"
expect "  its columns (the join columns kept)" "dataset cut run_id rule failed" \
  "$(tq qd-quality "DESCRIBE gold.quality_gold.v_rule_results" | cut -f1 | tr '\n' ' ' | sed 's/ $//')"

# step USER DATASET RUN SEQ STEP: one step of a run (1 insert, 2 flip, 3 delete old).
step() {
  local u=job_rules_$1 d=$1 run=$2 seq=$3 n t
  case $4 in
    1) tq "$u" "INSERT INTO $G.rule_results VALUES ('$d', $CUT, '$run', 'r1', 10 * $seq),
         ('$d', $CUT, '$run', 'r2', 20 * $seq), ('$d', $CUT, '$run', 'r3', 30 * $seq)" >/dev/null ;;
    2) tq "$u" "INSERT INTO $G.run_summary VALUES ('$d', $CUT, '$run', $seq, 3)" >/dev/null ;;
    3) for t in rule_results run_summary; do
         n=$(tq "$u" "SELECT count(*) FROM $G.$t WHERE dataset = '$d' AND cut = $CUT AND run_id <> '$run'")
         [ "$n" -gt 0 ] 2>/dev/null || continue
         tq "$u" "DELETE FROM $G.$t WHERE dataset = '$d' AND cut = $CUT AND run_id <> '$run'" >/dev/null ||
           ko "delete old in $t for $run"
       done ;;
  esac
}
seen() { tq person_loans "SELECT coalesce(max(run_id), '-') || ' ' || CAST(count(*) AS varchar) || ' ' ||
  CAST(coalesce(sum(failed), 0) AS varchar) FROM gold.quality_gold.v_rule_results"; }
stored() { tq qd-quality "SELECT array_join(array_agg(DISTINCT run_id ORDER BY run_id), ',') FROM $G.$1"; }

echo "== insert new, flip, delete old; a crash after each step"
for s in 1 2 3; do step loans r1 1 "$s"; done
expect "run r1 whole: a person sees" "r1 3 60" "$(seen)"
step loans r2 2 1
expect "crash after insert (r2): a person still sees r1 whole" "r1 3 60" "$(seen)"
expect "  r2's rows stored, invisible" "r1,r2" "$(stored rule_results)"
for s in 1 2 3; do step loans r3 3 "$s"; done
expect "the next run (r3) whole" "r3 3 180" "$(seen)"
expect "  its delete old removed r1 and the crashed r2" "r3" "$(stored rule_results)"
step loans r4 4 1; step loans r4 4 2
expect "crash after the flip (r4): a person sees r4 whole" "r4 3 240" "$(seen)"
expect "  r3 still stored" "r3,r4" "$(stored rule_results)"
step loans r4 4 3
expect "delete old re-run alone" "r4" "$(stored rule_results)"
step loans r4 4 3
expect "delete old a second time (nothing to delete, no DELETE sent)" "r4" "$(stored rule_results)"
expect "run_summary keeps the current run only" "r4" "$(stored run_summary)"
step loans r4 4 1
info "insert re-run with the same run_id after the flip: a person sees $(seen) (run id, rows, sum)"
step loans r5 5 1; step loans r5 5 2; step loans r5 5 3
expect "a new run after it" "r5 3 300" "$(seen)"

echo "== what the deletes wrote"
tq_ok "rule_results: files by content (0 data, 1 position deletes, 2 equality)" qd-quality \
  "SELECT content, count(*) FROM $G.\"rule_results\$files\" GROUP BY content ORDER BY content"
expect "rule_results: delete files" "0" "$(tq qd-quality "SELECT count(*) FROM $G.\"rule_results\$files\" WHERE content <> 0")"
tq_ok "rule_results: delete snapshots' summaries" qd-quality "SELECT operation,
  element_at(summary, 'deleted-data-files'), element_at(summary, 'added-delete-files')
  FROM $G.\"rule_results\$snapshots\" WHERE operation IN ('delete', 'overwrite') ORDER BY committed_at"
info "run_summary (partitioned by dataset only): delete files $(tq qd-quality \
  "SELECT count(*) FROM $G.\"run_summary\$files\" WHERE content <> 0")"
out=$(tq job_rules_loans "DELETE FROM $G.rule_results WHERE dataset = 'loans' AND cut = $CUT AND run_id = 'none'")
if [ $? -ne 0 ]; then ok "an empty DELETE refused: $(printf '%s' "$out" | head -c 200)"; else
  info "an empty DELETE accepted: '$out'"; fi
info "snapshots after the empty DELETE: $(tq qd-quality "SELECT count(*) FROM $G.\"rule_results\$snapshots\"")"

. tools/verify/trino_gold_replace_more.sh
[ "$FAILED" = 0 ] && echo "ALL OK" || echo "SOME CHECKS FAILED"
exit "$FAILED"
