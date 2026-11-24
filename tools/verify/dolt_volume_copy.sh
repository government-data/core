#!/usr/bin/env bash
# Whether the update agent's backup of a volume is a backup of Dolt's
# catalog (decision 0080, its amendment of 2026-11-24): quality-sdk's agent
# backs up a critical volume as `docker compose cp <service>:<path> -` of
# the running container, a tar of the files as they are, and restores it
# into the emptied volume of the stopped container. This checks that a
# copy of Dolt's data directory taken that way opens as the same catalog:
#
#   1. idle: the copy restores; `dolt fsck` passes; `main`'s commit, its
#      rows and the grants (the application's user signs in) are the same.
#   2. during writes and commits (the agent backs up in maintenance, when
#      the API writes nothing, so this is the worse case): the copy
#      restores, `dolt fsck` passes, and `main` is a commit the server had.
#   3. during a garbage collection (`auto_gc_behavior` is on): as 2.
#
# Its own containers, network and volumes, all `qd-verify-dolt-copy-*`,
# removed on exit. No port is published. Run alone (AGENTS.md).
set -euo pipefail

IMAGE=${DOLT_IMAGE:-dolthub/dolt-sql-server@sha256:24bdcff7a90bef6d15e8825be502f1c057842d079411390448dc13398c8bf619}
P=qd-verify-dolt-copy
WORK=$(mktemp -d)
ROOT_PW=root-$RANDOM$RANDOM
APP_PW=app-$RANDOM$RANDOM
DB=tenant_00000000000000000000000000000001
fails=0

cleanup() {
  docker ps -aq --filter "name=^$P-" | xargs -r docker rm -f >/dev/null 2>&1 || true
  docker volume ls -q --filter "name=^$P-" | xargs -r docker volume rm >/dev/null 2>&1 || true
  docker network rm "$P-net" >/dev/null 2>&1 || true
  rm -rf "$WORK"
}
trap cleanup EXIT
cleanup_quiet() { docker rm -f "$1" >/dev/null 2>&1 || true; }

pass() { echo "PASS: $*"; }
fail() { echo "FAIL: $*"; fails=$((fails + 1)); }

docker network create --internal "$P-net" >/dev/null

# A Dolt server on volume $2, as the container $1.
serve() {
  docker run -d --name "$1" --network "$P-net" -v "$2:/var/lib/dolt" \
    -e DOLT_ROOT_PASSWORD="$ROOT_PW" -e DOLT_ROOT_HOST=localhost "$IMAGE" >/dev/null
  for _ in $(seq 60); do
    if docker exec "$1" dolt sql -q "SELECT 1" >/dev/null 2>&1; then return 0; fi
    sleep 1
  done
  docker logs "$1" | tail -20
  return 1
}
sql() { docker exec -i "$1" dolt sql -r csv -q "$2" | tail -n +2; }

# The first server: the catalog's database, the application's user with a
# grant on it, a table and 50 commits.
serve "$P-src" "$P-src"
sql "$P-src" "CREATE DATABASE $DB;
  CREATE USER 'qd_app'@'%' IDENTIFIED BY '$APP_PW';
  GRANT ALL ON \`$DB\`.* TO 'qd_app'@'%';" >/dev/null
sql "$P-src" "USE $DB; CREATE TABLE rules (id INT PRIMARY KEY, body LONGTEXT NOT NULL);
  CALL DOLT_COMMIT('-Am', 'schema', '--author', 'verify <verify@example.invalid>');" >/dev/null
for i in $(seq 1 50); do
  sql "$P-src" "USE $DB; INSERT INTO rules VALUES ($i, REPEAT('x', 4000));
    CALL DOLT_COMMIT('-am', 'row $i', '--author', 'verify <verify@example.invalid>');" >/dev/null
done

# The copy, as the agent takes it: a tar of the running container's path.
copy() { docker cp "$P-src:/var/lib/dolt" - >"$WORK/$1.tar"; }

# The copy restored into a new volume, then served and checked.
restore_and_check() {
  local name=$1 expect_head=${2:-} vol="$P-$1" c="$P-$1-srv"
  docker volume create "$vol" >/dev/null
  # `docker cp -` of a directory is a tar whose top is `dolt/`.
  docker run --rm -i -v "$vol:/var/lib/dolt" --entrypoint sh "$IMAGE" -c \
    'cd /var/lib && tar -xf - && chown -R 0:0 /var/lib/dolt' <"$WORK/$name.tar"
  # Checked before a server holds it.
  if docker run --rm --network none -v "$vol:/var/lib/dolt" -w "/var/lib/dolt/$DB" \
    --entrypoint dolt "$IMAGE" fsck >"$WORK/$name.fsck" 2>&1; then
    pass "$name: dolt fsck"
  else
    fail "$name: dolt fsck: $(tail -3 "$WORK/$name.fsck" | tr '\n' ' ')"
  fi
  if ! serve "$c" "$vol"; then fail "$name: the restored copy does not start"; return; fi
  local head rows
  head=$(sql "$c" "USE $DB; SELECT HASHOF('main')" || true)
  rows=$(sql "$c" "USE $DB; SELECT count(*) FROM rules" || true)
  if [[ -n $expect_head ]]; then
    [[ $head == "$expect_head" ]] && pass "$name: main is the same commit" ||
      fail "$name: main is $head, not $expect_head"
  else
    grep -qx "$head" "$WORK/heads" && pass "$name: main ($rows rows) is a commit the server had" ||
      fail "$name: main $head is not a commit the server had"
  fi
  # The grants are in the same volume: the application's user signs in.
  if docker exec "$c" dolt --host 127.0.0.1 --port 3306 --no-tls -u qd_app -p "$APP_PW" \
    --use-db "$DB" sql -q "SELECT count(*) FROM rules" >/dev/null 2>&1; then
    pass "$name: qd_app signs in and reads"
  else
    fail "$name: qd_app does not sign in"
  fi
  cleanup_quiet "$c"
}

# 1. Idle.
head=$(sql "$P-src" "USE $DB; SELECT HASHOF('main')")
copy idle
restore_and_check idle "$head"

# A writer that commits as fast as it can, recording each head.
writer() {
  local i=1000
  while [[ -f $WORK/writing ]]; do
    i=$((i + 1))
    sql "$P-src" "USE $DB; INSERT INTO rules VALUES ($i, REPEAT('y', 4000));
      CALL DOLT_COMMIT('-am', 'row $i', '--author', 'verify <verify@example.invalid>');" >/dev/null 2>&1 || true
    sql "$P-src" "USE $DB; SELECT HASHOF('main')" >>"$WORK/heads" 2>/dev/null || true
  done
}
sql "$P-src" "USE $DB; SELECT commit_hash FROM dolt_log" >"$WORK/heads"

# 2. During writes: three copies, each restored.
touch "$WORK/writing"
writer &
wpid=$!
sleep 3
for n in 1 2 3; do copy "writes$n"; sleep 1; done
rm -f "$WORK/writing"
wait "$wpid" || true
sql "$P-src" "USE $DB; SELECT commit_hash FROM dolt_log" >>"$WORK/heads"
for n in 1 2 3; do restore_and_check "writes$n"; done

# 3. During garbage collections, with writes between them.
touch "$WORK/writing"
writer &
wpid=$!
( while [[ -f $WORK/writing ]]; do sql "$P-src" "USE $DB; CALL DOLT_GC()" >/dev/null 2>&1 || true; done ) &
gpid=$!
sleep 3
for n in 1 2 3; do copy "gc$n"; sleep 1; done
rm -f "$WORK/writing"
wait "$wpid" "$gpid" || true
sql "$P-src" "USE $DB; SELECT commit_hash FROM dolt_log" >>"$WORK/heads"
for n in 1 2 3; do restore_and_check "gc$n"; done

docker exec "$P-src" dolt version | head -1
if ((fails)); then
  echo "dolt_volume_copy: $fails check(s) failed"
  exit 1
fi
echo "dolt_volume_copy: all checks pass"
