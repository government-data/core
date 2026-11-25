#!/bin/bash
# Whether a lake that starts on one RustFS node can grow (decision 0091,
# point 3 and its V1): one node with one drive, objects written, then
#   (a) the single node alone: it serves writes and reads;
#   (b) more drives on the same node (RUSTFS_VOLUMES /data{1...4}, the old
#       drive as /data1): does it start, are the old objects readable, do
#       new writes land;
#   (c) a second pool beside it: the old node as pool 1 and four new nodes
#       as pool 2 (RUSTFS_VOLUMES "<pool 1> <pool 2>" on every node): the
#       same three questions;
#   (e) one node with four drives from the start (pool 1, erasure-coded on
#       one machine), objects written, then a pool of four nodes added;
#   (d) if (b) and (c) fail, the copy path: a new pool of four nodes as a
#       lake of its own, every object copied over S3 and checked by its
#       SHA-256 (bucket replication from a single node into another lake
#       is lake_replication_sync.sh's, verified 2026-11-24).
# Every drive here is a directory of one host disk, so the cases with
# several drives on one node set RUSTFS_UNSAFE_BYPASS_DISK_CHECK=true, as
# RustFS asks for tests only; on a server each drive is its own disk.
# The original node's drive is restored from a copy before each case, so
# one case cannot spoil the next.
#
# Throwaway containers only (qd-verify-grow-…, on a network of their own,
# no port published), random root keys never printed, synthetic objects.
# Run alone (no build at the same time), from the repository root:
#
#   bash tools/verify/lake_single_node_grow.sh
#
# Needs docker, curl 7.75 or later (--aws-sigv4), openssl and sha256sum.
set -u
export LC_ALL=C
IMAGE=${RUSTFS_IMAGE:-rustfs/rustfs:1.0.5}
P=qd-verify-grow
WAIT=${QD_VERIFY_WAIT:-60}
N=${QD_VERIFY_OBJECTS:-20}
D=$(mktemp -d)
chmod 700 "$D"
cleanup() {
  docker ps -aq --filter "name=^$P-" | xargs -r docker rm -f >/dev/null 2>&1
  docker network rm "$P" >/dev/null 2>&1
  # The drives made RustFS's are not ours to remove.
  docker run --rm -v "$D:/d" alpine:3.20 sh -c 'rm -rf /d/drives' >/dev/null 2>&1
  rm -rf "$D"
}
trap cleanup EXIT
FAILED=0
ok() { printf '[OK  ] %s\n' "$*"; }
ko() {
  printf '[FAIL] %s\n' "$*"
  FAILED=1
}
note() { printf '[INFO] %s\n' "$*"; }

docker network create "$P" >/dev/null || exit 1
printf 'RUSTFS_ACCESS_KEY=qd%s\nRUSTFS_SECRET_KEY=%s\n' "$(openssl rand -hex 8)" "$(openssl rand -hex 20)" >"$D/root.env"
chmod 600 "$D/root.env"
(
  # shellcheck source=/dev/null
  . "$D/root.env"
  printf 'user = "%s:%s"\naws-sigv4 = "aws:amz:us-east-1:s3"\n' "$RUSTFS_ACCESS_KEY" "$RUSTFS_SECRET_KEY"
) >"$D/curl.cfg"
chmod 600 "$D/curl.cfg"
mkdir -p "$D/drives" "$D/objects"
for i in $(seq 1 "$N"); do openssl rand -out "$D/objects/o$i" $((4096 + i * 997)); done

ip() { docker inspect -f "{{(index .NetworkSettings.Networks \"$P\").IPAddress}}" "$1" 2>/dev/null; }
code() { # NODE METHOD PATH [FILE]
  local n=$1 m=$2 p=$3 f=${4:-}
  if [ -n "$f" ]; then
    curl -s -o /dev/null -w '%{http_code}' --max-time 15 -K "$D/curl.cfg" -X "$m" --data-binary "@$f" "http://$(ip "$n"):9000$p" 2>/dev/null
  else
    curl -s -o /dev/null -w '%{http_code}' --max-time 15 -K "$D/curl.cfg" -X "$m" "http://$(ip "$n"):9000$p" 2>/dev/null
  fi
}
get() { curl -s --max-time 15 -K "$D/curl.cfg" -o "$3" "http://$(ip "$1"):9000$2" 2>/dev/null; }
drive() { # NAME: an empty drive owned by RustFS's user (uid 10001)
  mkdir -p "$D/drives/$1"
  docker run --rm -v "$D/drives/$1:/x" alpine:3.20 chown 10001:10001 /x >/dev/null
}
run_node() { # NAME VOLUMES MOUNTS...
  local n=$1 v=$2
  shift 2
  local m=()
  for x in "$@"; do m+=(-v "$D/drives/$x"); done
  docker run -d --name "$n" --hostname "$n" --network "$P" --env-file "$D/root.env" \
    -e "RUSTFS_VOLUMES=$v" ${BYPASS:+-e RUSTFS_UNSAFE_BYPASS_DISK_CHECK=true} ${EXTRA:-} "${m[@]}" "$IMAGE" server --address :9000 >/dev/null
}
ready() { # NODE: waits until a signed ListBuckets answers 200
  local t=0
  while [ $t -lt "$WAIT" ]; do
    [ "$(docker inspect -f '{{.State.Running}}' "$1" 2>/dev/null)" = true ] || { echo exited; return 1; }
    [ "$(code "$1" GET /)" = 200 ] && { echo ready; return 0; }
    sleep 2
    t=$((t + 2))
  done
  echo "not ready after ${WAIT}s"
  return 1
}
old_readable() { # NODE: how many of the first-written objects read back intact
  local good=0 i
  for i in $(seq 1 "$N"); do
    get "$1" "/grow/o$i" "$D/back" && [ "$(sha256sum <"$D/back")" = "$(sha256sum <"$D/objects/o$i")" ] && good=$((good + 1))
  done
  echo "$good"
}
new_writes() { # NODE TAG: how many new objects are written and read back
  local good=0 i
  for i in 1 2 3 4 5; do
    [ "$(code "$1" PUT "/grow/new-$2-$i" "$D/objects/o$i")" = 200 ] || continue
    get "$1" "/grow/new-$2-$i" "$D/back" && cmp -s "$D/back" "$D/objects/o$i" && good=$((good + 1))
  done
  echo "$good"
}
logtail() { docker logs --tail 4 "$1" 2>&1 | sed 's/^/         /' | cut -c1-600; }
down() { docker ps -aq --filter "name=^$P-$1" | xargs -r docker rm -f >/dev/null 2>&1; }
restore_one() { # the original drive back to its state after (a), into one-<case>
  docker run --rm -v "$D/drives:/d" alpine:3.20 sh -c "rm -rf /d/one-$1 && cp -a /d/one-saved /d/one-$1" >/dev/null
}

echo "== (a) one node, one drive"
drive one
run_node "$P-a" /data one:/data
r=$(ready "$P-a")
if [ "$r" != ready ]; then
  ko "the single node starts: $r"
  logtail "$P-a"
  exit 1
fi
ok "the single node starts with RUSTFS_VOLUMES=/data"
[ "$(code "$P-a" PUT /grow)" = 200 ] && ok "bucket made" || ko "bucket made"
w=0
for i in $(seq 1 "$N"); do [ "$(code "$P-a" PUT "/grow/o$i" "$D/objects/o$i")" = 200 ] && w=$((w + 1)); done
[ "$w" = "$N" ] && ok "$N objects written" || ko "objects written: $w of $N"
g=$(old_readable "$P-a")
[ "$g" = "$N" ] && ok "$N objects read back intact" || ko "read back: $g of $N"
docker stop -t 5 "$P-a" >/dev/null
docker rm "$P-a" >/dev/null
docker run --rm -v "$D/drives:/d" alpine:3.20 cp -a /d/one /d/one-saved >/dev/null

echo "== (b) more drives on the same node (the old drive as /data1)"
restore_one b
for k in 2 3 4; do drive "b$k"; done
BYPASS=1 run_node "$P-b" "/data{1...4}" one-b:/data1 b2:/data2 b3:/data3 b4:/data4
r=$(ready "$P-b")
B=no
if [ "$r" = ready ]; then
  g=$(old_readable "$P-b")
  nw=$(new_writes "$P-b" b)
  note "started; old objects intact: $g of $N; new writes: $nw of 5"
  [ "$g" = "$N" ] && [ "$nw" = 5 ] && B=yes
else
  note "does not start: $r"
  logtail "$P-b"
fi
[ $B = yes ] && ok "(b) the node grows by drives in place" || note "(b) the node does not grow by drives in place"
down b

echo "== (c) a second pool of four nodes beside the old node"
restore_one c
for k in 1 2 3 4; do drive "c$k"; done
VOLS="http://$P-c0:9000/data http://$P-c{1...4}:9000/data"
run_node "$P-c0" "$VOLS" one-c:/data
for k in 1 2 3 4; do run_node "$P-c$k" "$VOLS" "c$k:/data"; done
r=$(ready "$P-c0")
C=no
if [ "$r" = ready ]; then
  g=$(old_readable "$P-c0")
  g3=$(old_readable "$P-c3")
  nw=$(new_writes "$P-c0" c)
  note "started; old objects intact on the old node: $g of $N, through a new node: $g3 of $N; new writes: $nw of 5"
  [ "$g" = "$N" ] && [ "$g3" = "$N" ] && [ "$nw" = 5 ] && C=yes
else
  note "does not start: $r"
  logtail "$P-c0"
  logtail "$P-c1"
fi
[ $C = yes ] && ok "(c) a pool can be added to the single node" || note "(c) a pool cannot be added to the single node"
down c

echo "== (e) one node with four drives, then a pool of four nodes added"
for k in 1 2 3 4; do drive "e0-$k"; done
E1="http://$P-e0:9000/data{1...4}"
BYPASS=1 run_node "$P-e0" "$E1" e0-1:/data1 e0-2:/data2 e0-3:/data3 e0-4:/data4
E=no
r=$(ready "$P-e0")
if [ "$r" = ready ]; then
  [ "$(code "$P-e0" PUT /grow)" = 200 ] || ko "(e) bucket made"
  w=0
  for i in $(seq 1 "$N"); do [ "$(code "$P-e0" PUT "/grow/o$i" "$D/objects/o$i")" = 200 ] && w=$((w + 1)); done
  note "(e) one node, four drives: $w of $N objects written"
  docker stop -t 5 "$P-e0" >/dev/null
  docker rm "$P-e0" >/dev/null
  for k in 1 2 3 4; do drive "e$k"; done
  # The image's entrypoint makes every path of RUSTFS_VOLUMES on every
  # node, so a new node gets empty tmpfs for the paths it does not serve.
  VOLS="$E1 http://$P-e{1...4}:9000/data1"
  BYPASS=1 run_node "$P-e0" "$VOLS" e0-1:/data1 e0-2:/data2 e0-3:/data3 e0-4:/data4
  T="--tmpfs /data2:uid=10001,gid=10001 --tmpfs /data3:uid=10001,gid=10001 --tmpfs /data4:uid=10001,gid=10001"
  for k in 1 2 3 4; do EXTRA=$T run_node "$P-e$k" "$VOLS" "e$k:/data1"; done
  r=$(ready "$P-e0")
  if [ "$r" = ready ]; then
    g=$(old_readable "$P-e0")
    g3=$(old_readable "$P-e3")
    nw=$(new_writes "$P-e2" e)
    note "(e) grown; old objects intact on the old node: $g of $N, through a new node: $g3 of $N; new writes: $nw of 5"
    [ "$g" = "$N" ] && [ "$g3" = "$N" ] && [ "$nw" = 5 ] && E=yes
  else
    note "(e) the grown lake does not start: $r"
    logtail "$P-e0"
    logtail "$P-e1"
  fi
else
  note "(e) one node with four drives does not start: $r"
  logtail "$P-e0"
fi
[ $E = yes ] && ok "(e) a pool can be added to a one-node, four-drive lake" || note "(e) a pool cannot be added to a one-node, four-drive lake"
down e

echo "== (d) the copy path: a new lake of four nodes, every object copied"
restore_one d
run_node "$P-d0" /data one-d:/data
for k in 1 2 3 4; do drive "d$k"; done
for k in 1 2 3 4; do run_node "$P-d$k" "http://$P-d{1...4}:9000/data" "d$k:/data"; done
r0=$(ready "$P-d0")
r1=$(ready "$P-d1")
if [ "$r0" = ready ] && [ "$r1" = ready ]; then
  [ "$(code "$P-d1" PUT /grow)" = 200 ] || ko "bucket made on the new lake"
  c=0
  for i in $(seq 1 "$N"); do
    get "$P-d0" "/grow/o$i" "$D/copy" && [ "$(code "$P-d1" PUT "/grow/o$i" "$D/copy")" = 200 ] && c=$((c + 1))
  done
  g=$(old_readable "$P-d3")
  nw=$(new_writes "$P-d2" d)
  if [ "$c" = "$N" ] && [ "$g" = "$N" ] && [ "$nw" = 5 ]; then
    ok "(d) $N objects copied into a four-node lake, intact through another node; new writes $nw of 5"
  else
    ko "(d) copied $c, intact $g of $N, new writes $nw of 5"
  fi
else
  ko "(d) the lakes start: old $r0, new $r1"
fi
down d

echo "== result"
echo "one drive: grows by drives in place: $B; by adding a pool: $C. Four drives on one node: by adding a pool: $E"
echo "run $(date -u +%Y-%m-%dT%H:%MZ) with $IMAGE ($(docker image inspect -f '{{.Id}}' "$IMAGE" | cut -c1-19))"
exit $FAILED
