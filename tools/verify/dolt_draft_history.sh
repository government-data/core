#!/bin/sh
# Verifies what a draft's history needs from Dolt (decision 0021): the
# commits of a branch that `main` has not (`DOLT_LOG('main..<branch>')`),
# where the branch left `main` (`DOLT_MERGE_BASE`), and a rules file's
# change between two commits (`DOLT_DIFF(<from>, <to>, 'rules')`).
#
# Found (2026-11-01, Dolt 2.6.1): all three as expected; the diff from the
# merge base leaves out what `main` changed after the branch. Through sqlx,
# DOLT_LOG takes its range as one bound value but not an expression
# (`CONCAT('main..', ?)` is refused), and DOLT_DIFF takes no bound value at
# all: its commits go in as literals, checked first.
#
# Runs in the dev stack's Dolt container, on a scratch repository of its own
# under /tmp (not the server's databases), removed at the end; synthetic
# rows only.
#
#   sh tools/verify/dolt_draft_history.sh
set -eu
C=${DOLT_CONTAINER:-data-quality-dolt-dolt-1}
DB=verify_draft_history

q() { docker exec "$C" sh -c "cd /tmp/$DB && dolt sql -r csv -q \"$1\""; }

docker exec "$C" sh -c "rm -rf /tmp/$DB && mkdir /tmp/$DB && cd /tmp/$DB && dolt init --name verify --email verify@example.invalid" >/dev/null

q "CREATE TABLE rules (dataset VARCHAR(128), file VARCHAR(128), body LONGTEXT, PRIMARY KEY (dataset, file));
   INSERT INTO rules VALUES ('d', 'a.yaml', 'v1'), ('d', 'b.yaml', 'b1');
   CALL DOLT_COMMIT('-Am', 'main: first', '--author', 'Main <main@example.invalid>');
   CALL DOLT_BRANCH('proposal/d/x');
   CALL DOLT_CHECKOUT('proposal/d/x');
   UPDATE rules SET body = 'v2' WHERE file = 'a.yaml';
   CALL DOLT_COMMIT('-am', 'edit one', '--author', 'Ana <ana@example.invalid>');
   UPDATE rules SET body = 'v3' WHERE file = 'a.yaml';
   CALL DOLT_COMMIT('-am', 'edit two', '--author', 'Ben <ben@example.invalid>');
   CALL DOLT_CHECKOUT('main');
   UPDATE rules SET body = 'b2' WHERE file = 'b.yaml';
   CALL DOLT_COMMIT('-am', 'main moves on', '--author', 'Main <main@example.invalid>');" >/dev/null

echo "== DOLT_LOG('main..proposal/d/x'): the draft's own commits, newest first"
q "SELECT commit_hash, committer, email, message FROM DOLT_LOG('main..proposal/d/x')"
echo "== DOLT_LOG('main..proposal/d/x', '--parents')"
q "SELECT message, parents FROM DOLT_LOG('main..proposal/d/x', '--parents')"
echo "== DOLT_MERGE_BASE: where it left main"
q "SELECT DOLT_MERGE_BASE('main', 'proposal/d/x') = (SELECT commit_hash FROM DOLT_LOG('main') WHERE message = 'main: first') AS base_is_first"
echo "== DOLT_DIFF(base, branch, 'rules'): the whole draft (main's later change not in it)"
q "SELECT from_file, to_file, from_body, to_body, diff_type FROM DOLT_DIFF(DOLT_MERGE_BASE('main', 'proposal/d/x'), 'proposal/d/x', 'rules')"
echo "== DOLT_DIFF(commit~, commit, 'rules'): one commit"
q "SELECT from_body, to_body FROM DOLT_DIFF('proposal/d/x~', 'proposal/d/x', 'rules')"

docker exec "$C" sh -c "rm -rf /tmp/$DB"
