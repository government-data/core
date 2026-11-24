# shellcheck shell=bash
# The second half of tools/verify/trino_gold_replace.sh (decision 0090,
# T2), sourced by it: row filters through the views, a pinned snapshot,
# CREATE BRANCH and DROP TABLE.

echo "== row filters through SECURITY INVOKER views over the READ_ONLY catalog"
for s in 1 2 3; do step savings s1 7 "$s"; done
expect "qd-quality sees both datasets in the view" "loans savings" \
  "$(tq qd-quality "SELECT array_join(array_agg(DISTINCT dataset ORDER BY dataset), ' ') FROM gold.quality_gold.v_rule_results")"
expect "person_loans sees only loans in the view" "loans" \
  "$(tq person_loans "SELECT array_join(array_agg(DISTINCT dataset), ' ') FROM gold.quality_gold.v_rule_results")"
expect "person_loans: count(*) on the table" "3" "$(tq person_loans "SELECT count(*) FROM gold.quality_gold.rule_results")"
expect "person_loans: current_run" "loans" "$(tq person_loans "SELECT array_join(array_agg(dataset), ' ') FROM gold.quality_gold.current_run")"
tq_refused "person_loans: INSERT through gold" person_loans \
  "INSERT INTO gold.quality_gold.rule_results VALUES ('loans', $CUT, 'x', 'r1', 1)"
tq_refused "person_loans: the lake catalog" person_loans "SELECT count(*) FROM $G.rule_results"
tq_refused "job_rules_loans: INSERT of a savings row" job_rules_loans \
  "INSERT INTO $G.rule_results VALUES ('savings', $CUT, 'x', 'r1', 1)"
expect "job_rules_loans: DELETE sees only loans (savings rows kept)" "3" \
  "$(tq job_rules_loans "DELETE FROM $G.rule_results WHERE run_id = 's1'" >/dev/null;
     tq qd-quality "SELECT count(*) FROM $G.rule_results WHERE dataset = 'savings'")"

echo "== FOR VERSION AS OF a \$refs snapshot"
snap=$(tq job_rules_loans "SELECT snapshot_id FROM $G.\"rule_results\$refs\" WHERE name = 'main'")
expect_match "main's snapshot read from \$refs" '^[0-9-]+$' "$snap"
before=$(tq job_rules_loans "SELECT count(*) FROM $G.rule_results FOR VERSION AS OF $snap")
step loans r6 6 1
expect "pinned read unchanged after a later insert" "$before" \
  "$(tq job_rules_loans "SELECT count(*) FROM $G.rule_results FOR VERSION AS OF $snap")"
expect "unpinned read sees the insert" "$((before + 3))" "$(tq job_rules_loans "SELECT count(*) FROM $G.rule_results")"
expect "pinned read through gold (person_loans)" "$before" \
  "$(tq person_loans "SELECT count(*) FROM gold.quality_gold.rule_results FOR VERSION AS OF $snap")"

echo "== CREATE BRANCH and DROP TABLE"
tq_refused "CREATE BRANCH as job_rules_loans" job_rules_loans "CREATE BRANCH audit IN TABLE $G.rule_results"
out=$(tq qd-quality "CREATE BRANCH audit IN TABLE $G.rule_results")
info "CREATE BRANCH as qd-quality (rc $?): $(printf '%s' "$out" | head -c 200)"
info "  (with every user allowed, Trino 484 answers: This connector does not support creating branches)"
tq_refused "CREATE BRANCH through gold" qd-quality "CREATE BRANCH audit2 IN TABLE gold.quality_gold.rule_results"
tq_ok "a throwaway table" qd-quality "CREATE TABLE $G.throwaway AS SELECT 1 AS a"
tq_refused "DROP TABLE on RustFS" qd-quality "DROP TABLE $G.throwaway"
expect "  the REST catalog's answer to the purge Trino asks for" 406 \
  "$(te_lake_call DELETE "/iceberg/v1/$WAREHOUSE/namespaces/quality_gold/tables/throwaway?purgeRequested=true" "" admin)"
info "  the same without purge: $(te_lake_call DELETE "/iceberg/v1/$WAREHOUSE/namespaces/quality_gold/tables/throwaway" "" admin)"
