#!/bin/sh
# Verifies what a draft per column needs from Dolt (decision 0050): two
# drafts, each changing one field's rules file, approved one after the
# other without conflict; whether `main` changed one file since a draft
# left it (`DOLT_DIFF` filtered by file); and a file's history on `main`
# (`dolt_history_rules`, walked along `main`'s first parents).
#
# Found (2026-11-04, Dolt 2.6.1): after a's draft is approved, b's draft
# left a `main` whose b.yaml has not changed since (0) while a.yaml has (1),
# and b merges after a without conflict. `dolt_history_rules` has a row for
# every commit with the file, branches' commits included, and the
# `dolt_diff_rules` of a merge is not the change it brought; DOLT_LOG has
# no `--first-parent`. So a file's history on `main` walks `main` by first
# parents (`DOLT_LOG('main', '--parents')`, the first of each) and marks
# where the file there (`dolt_history_rules`) differs from its parent's:
# here _approve a_ and _main: first_, not _approve b_.
#
# Runs in the dev stack's Dolt container, on a scratch repository of its own
# under /tmp (not the server's databases), removed at the end; synthetic
# rows only.
#
#   sh tools/verify/dolt_column_drafts.sh
set -eu
C=${DOLT_CONTAINER:-data-quality-dolt-dolt-1}
DB=verify_column_drafts

q() { docker exec "$C" sh -c "cd /tmp/$DB && dolt sql -r csv -q \"$1\""; }

docker exec "$C" sh -c "rm -rf /tmp/$DB && mkdir /tmp/$DB && cd /tmp/$DB && dolt init --name verify --email verify@example.invalid" >/dev/null

q "CREATE TABLE rules (dataset VARCHAR(128), file VARCHAR(128), body LONGTEXT, PRIMARY KEY (dataset, file));
   INSERT INTO rules VALUES ('d', 'a.yaml', 'a1'), ('d', 'b.yaml', 'b1');
   CALL DOLT_COMMIT('-Am', 'main: first', '--author', 'Main <main@example.invalid>');
   CALL DOLT_BRANCH('proposal/d/a');
   CALL DOLT_BRANCH('proposal/d/b');
   CALL DOLT_CHECKOUT('proposal/d/a');
   UPDATE rules SET body = 'a2' WHERE file = 'a.yaml';
   CALL DOLT_COMMIT('-am', 'a: two', '--author', 'Ana <ana@example.invalid>');
   CALL DOLT_CHECKOUT('proposal/d/b');
   UPDATE rules SET body = 'b2' WHERE file = 'b.yaml';
   CALL DOLT_COMMIT('-am', 'b: two', '--author', 'Ben <ben@example.invalid>');
   CALL DOLT_CHECKOUT('main');
   CALL DOLT_MERGE('proposal/d/a', '--no-ff', '-m', 'approve a', '--author', 'Rev <rev@example.invalid>');" >/dev/null

echo "== after approving a: did main change b.yaml since b's draft left it? (expect 0)"
q "SELECT count(*) AS b_changed FROM DOLT_DIFF(DOLT_MERGE_BASE('proposal/d/b', 'main'), 'main', 'rules') WHERE (from_dataset = 'd' AND from_file = 'b.yaml') OR (to_dataset = 'd' AND to_file = 'b.yaml')"
echo "== and a.yaml? (expect 1)"
q "SELECT count(*) AS a_changed FROM DOLT_DIFF(DOLT_MERGE_BASE('proposal/d/b', 'main'), 'main', 'rules') WHERE (from_dataset = 'd' AND from_file = 'a.yaml') OR (to_dataset = 'd' AND to_file = 'a.yaml')"
echo "== b approved after a: no conflict, both on main"
q "CALL DOLT_MERGE('proposal/d/b', '--no-ff', '-m', 'approve b', '--author', 'Rev <rev@example.invalid>')"
q "SELECT file, body FROM rules ORDER BY file"
echo "== a.yaml's history on main: dolt_history_rules, one row per commit that has it"
q "SELECT commit_hash, committer, commit_date, body FROM dolt_history_rules WHERE dataset = 'd' AND file = 'a.yaml' ORDER BY commit_date DESC"
echo "== main's line, walked by first parents from its tip (no --first-parent in DOLT_LOG):"
echo "   each commit, a.yaml there (dolt_history_rules), and whether it changed it"
q "SELECT commit_hash, parents, message, committer FROM DOLT_LOG('main', '--parents')" > /tmp/qd_log.$$
q "SELECT commit_hash, body FROM dolt_history_rules WHERE dataset = 'd' AND file = 'a.yaml'" > /tmp/qd_hist.$$
python3 - /tmp/qd_log.$$ /tmp/qd_hist.$$ <<'PY'
import csv, sys
log = list(csv.DictReader(open(sys.argv[1])))
body = {r["commit_hash"]: r["body"] for r in csv.DictReader(open(sys.argv[2]))}
first = {r["commit_hash"]: (r["parents"].split(", ")[0] if r["parents"] else "") for r in log}
info = {r["commit_hash"]: r for r in log}
h = log[0]["commit_hash"]
while h:
    b, pb = body.get(h, "-"), body.get(first[h], "-")
    print(f'{h[:8]},{info[h]["message"]},{info[h]["committer"]},{b},{"changed" if b != pb else ""}')
    h = first[h]
PY
rm -f /tmp/qd_log.$$ /tmp/qd_hist.$$

docker exec "$C" sh -c "rm -rf /tmp/$DB"
