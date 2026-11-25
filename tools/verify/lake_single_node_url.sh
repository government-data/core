#!/bin/bash
# Whether RustFS starts a one-node pool as the core lays it out (decision
# 0091, step 3): a pool node's RUSTFS_VOLUMES names every drive by URL,
# `http(s)://<node>:<port>/data/<n>`, one by one (auth-gateway's
# PoolLayout::volumes), never a local path. lake_single_node_grow.sh (V1)
# checked one drive as the path `/data`, and four drives on one node as
# `http://<node>:9000/data{1...4}`; this checks the core's own form:
#   (u1) one node, one drive: RUSTFS_VOLUMES="http://<node>:9000/data/1";
#   (p1) the same drive as the path "/data/1", as join.sh mounts it;
#   (u4) one node, four drives, listed one by one.
# Each case: does it start, and do objects written read back intact. (u1)
# is expected to be refused (RustFS asks for a path on a single drive).
# Every drive here is a directory of one host disk, so (u4) sets
# RUSTFS_UNSAFE_BYPASS_DISK_CHECK=true, as RustFS asks for tests only.
#
# Throwaway containers only (qd-verify-url-…, on a network of their own,
# no port published), random root keys never printed, synthetic objects.
# Run alone (no build at the same time), from the repository root:
#
#   bash tools/verify/lake_single_node_url.sh
#
# Needs docker, curl 7.75 or later (--aws-sigv4) and openssl.
set -u
export LC_ALL=C
IMAGE=${RUSTFS_IMAGE:-rustfs/rustfs:1.0.5}
docker image inspect "$IMAGE" -f 'image: {{index .RepoDigests 0}}' 2>/dev/null || true
P=qd-verify-url
WAIT=${QD_VERIFY_WAIT:-60}
N=10
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
  local f=()
  [ -z "${4:-}" ] || f=(--data-binary "@$4")
  curl -s -o /dev/null -w '%{http_code}' --max-time 15 -K "$D/curl.cfg" -X "$2" "${f[@]}" "http://$(ip "$1"):9000$3" 2>/dev/null
}
drive() { # NAME: an empty drive owned by RustFS's user (uid 10001)
  mkdir -p "$D/drives/$1"
  docker run --rm -v "$D/drives/$1:/x" alpine:3.20 chown 10001:10001 /x >/dev/null
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
# case_node TAG DRIVES [path]: one node named $P-TAG with DRIVES drives by
# URL, or by path.
case_node() {
  local n=$P-$1 drives=$2 v='' m=() k
  for k in $(seq 1 "$drives"); do
    drive "$1-$k"
    if [ "${3:-}" = path ]; then v="$v /data/$k"; else v="$v http://$n:9000/data/$k"; fi
    m+=(-v "$D/drives/$1-$k:/data/$k")
  done
  v=${v# }
  local bypass=()
  [ "$drives" = 1 ] || bypass=(-e RUSTFS_UNSAFE_BYPASS_DISK_CHECK=true)
  echo "== ($1) one node, $drives drive(s): RUSTFS_VOLUMES=\"$v\""
  docker run -d --name "$n" --hostname "$n" --network "$P" --env-file "$D/root.env" \
    -e "RUSTFS_VOLUMES=$v" "${bypass[@]}" "${m[@]}" "$IMAGE" server --address :9000 >/dev/null
  local r
  r=$(ready "$n")
  if [ "$1" = u1 ]; then
    if [ "$r" = exited ] && docker logs "$n" 2>&1 | grep -q "use path style endpoint for single node setup"; then
      ok "(u1) refused: \"use path style endpoint for single node setup\""
    else
      ko "(u1) refused as a URL: $r"
    fi
    docker rm -f "$n" >/dev/null
    return
  fi
  if [ "$r" != ready ]; then
    ko "($1) starts: $r"
    docker logs --tail 4 "$n" 2>&1 | sed 's/^/         /' | cut -c1-600
    return
  fi
  ok "($1) starts"
  [ "$(code "$n" PUT /url)" = 200 ] || ko "($1) bucket made"
  local good=0 i
  for i in $(seq 1 "$N"); do
    [ "$(code "$n" PUT "/url/o$i" "$D/objects/o$i")" = 200 ] || continue
    curl -s --max-time 15 -K "$D/curl.cfg" -o "$D/back" "http://$(ip "$n"):9000/url/o$i" &&
      cmp -s "$D/back" "$D/objects/o$i" && good=$((good + 1))
  done
  [ "$good" = "$N" ] && ok "($1) $N objects written and read back intact" || ko "($1) read back: $good of $N"
  docker rm -f "$n" >/dev/null
}

case_node u1 1
case_node p1 1 path
case_node u4 4
[ "$FAILED" = 0 ] && echo "PASS" || echo "FAIL"
exit "$FAILED"
