#!/bin/bash
# RustFS's object caches (decision 0028, the user's ruling of 2026-11-17:
# "enable both caches, verified first"): what the metadata cache
# (RUSTFS_OBJECT_CACHE_ENABLE) and the data cache
# (RUSTFS_OBJECT_DATA_CACHE_ENABLE and its cap) do, before the lake's
# composes turn them on.
#
#   (a) the settings: what RustFS logs at startup for each, and what it
#       resolves a cap to (bytes, a share of the container's memory);
#   (b) latency, caches off, metadata only, both: repeated GET of 4 KiB,
#       1, 16 and 64 MiB objects, HEAD, a listing of 200 objects; the
#       container's memory after reading 200 MiB three times;
#   (c) the cap: the cache's own count (admin stats) and the container's
#       memory against it;
#   (d) never stale, caches on: read-after-overwrite (PUT, CopyObject,
#       multipart), read-after-delete, a versioned bucket (latest, by
#       version, delete marker), each read warmed before the change;
#   (e) the same across a pool of 4 nodes, each with both caches: read on
#       node 2, changed through node 1, read again on node 2.
#
# Throwaway containers only (qd-verify-cache-…, ports on 127.0.0.1, no
# restart policy, removed with their volume and network on exit), random
# root keys that are never printed, synthetic random objects. Run alone
# (no build, load under 4), from the repository root:
#
#   bash tools/verify/rustfs_cache.sh
#
# Needs docker, curl 7.75 or later (--aws-sigv4), jq and openssl.
set -u
export LC_ALL=C
IMAGE=${RUSTFS_IMAGE:-rustfs/rustfs:1.0.5}
P=qd-verify-cache
MEM=${QD_VERIFY_MEMORY:-1g} # each container's memory limit
ROUNDS=${QD_VERIFY_ROUNDS:-12} # of the cap's reads (c)
D=$(mktemp -d)
chmod 700 "$D"
cleanup() {
  docker ps -aq --filter "name=^$P-" | xargs -r docker rm -f -v >/dev/null 2>&1
  docker volume ls -q --filter "name=$P-" | xargs -r docker volume rm >/dev/null 2>&1
  docker network rm "$P" >/dev/null 2>&1
  rm -rf "$D"
}
trap cleanup EXIT
FAILED=0
ok() { printf '[OK  ] %s\n' "$*"; }
ko() {
  printf '[FAIL] %s\n' "$*"
  FAILED=1
}
expect() { if [ "$2" = "$3" ]; then ok "$1: $3"; else ko "$1: expected '$2', got '$3'"; fi; }

printf 'RUSTFS_ACCESS_KEY=qd%s\nRUSTFS_SECRET_KEY=%s\n' "$(openssl rand -hex 8)" "$(openssl rand -hex 16)" >"$D/keys.env"
chmod 600 "$D/keys.env"
(
  . "$D/keys.env"
  printf 'user = "%s:%s"\naws-sigv4 = "aws:amz:us-east-1:s3"\nsilent\nmax-time = 60\n' "$RUSTFS_ACCESS_KEY" "$RUSTFS_SECRET_KEY"
) >"$D/curl.cfg"
chmod 600 "$D/curl.cfg"
s3() { curl -K "$D/curl.cfg" "$@" 2>/dev/null; }
code() { s3 -o /dev/null -w '%{http_code}' "$@"; }
sha() { s3 "$@" | sha256sum | cut -c1-16; }
fsha() { sha256sum "$1" | cut -c1-16; }
url() { echo "http://127.0.0.1:$(docker port "$1" 9000/tcp | head -1 | cut -d: -f2)"; }
ready() { # URL: within 60 s, ListBuckets answers 200
  for _ in $(seq 60); do [ "$(code "$1/")" = 200 ] && return 0; sleep 1; done
  return 1
}
med() { # N ARGS...: the median time_total in ms of N requests
  local n=$1
  shift
  for _ in $(seq "$n"); do s3 -o /dev/null -w '%{time_total}\n' "$@"; done |
    sort -n | awk '{a[NR]=$1} END {printf "%.1f", a[int((NR+1)/2)]*1000}'
}
mem() { # CONTAINER: its cgroup's process memory (anon) and page cache (file)
  docker exec "$1" cat /sys/fs/cgroup/memory.stat |
    awk '/^anon /{a=$2} /^file /{f=$2} END {printf "anon %d MiB (file %d)", a/1048576, f/1048576}'
}
stats() { s3 "$1/rustfs/admin/v3/object-data-cache/stats"; }
logged() { # CONTAINER: the cache lines of its log
  { docker logs "$1" 2>&1; docker exec "$1" sh -c 'cat /logs/*.log 2>/dev/null'; } | grep -i 'object.cache\|object data cache\|OBJECT_CACHE\|OBJECT_DATA_CACHE' |
    sed -E 's/"(timestamp|time|target|span|spans|threadId|threadName)":"?[^,]*,?//g' | cut -c1-400 | sort -u | head -8
}

echo "== Synthetic objects"
mkdir -m 700 "$D/o"
for s in 1 4 16 64; do head -c $((s << 20)) /dev/urandom >"$D/o/b$s"; done
for i in $(seq -w 1 16); do head -c $((8 << 20)) /dev/urandom >"$D/o/m$i"; done
head -c 4096 /dev/urandom >"$D/o/s"
ok "4 KiB, 1, 4, 16, 64 MiB, and 16 of 8 MiB (128 MiB)"

# run NAME ENV...: a single node on the shared volume, port on 127.0.0.1
run() {
  local n=$P-$1
  shift
  docker rm -f -v "$n" >/dev/null 2>&1
  docker run -d --name "$n" --memory "$MEM" --env-file "$D/keys.env" -e RUSTFS_VOLUMES=/data \
    -e RUSTFS_OBS_LOGGER_LEVEL=info "$@" -v "$P-data:/data" -p 127.0.0.1::9000 "$IMAGE" server --address :9000 >/dev/null
  U=$(url "$n")
  ready "$U" || { ko "$n did not start"; docker logs --tail 20 "$n"; exit 1; }
}
META=(-e RUSTFS_OBJECT_CACHE_ENABLE=true -e RUSTFS_OBJECT_CACHE_TTL_SECS=300)
DATA=(-e RUSTFS_OBJECT_DATA_CACHE_ENABLE=true -e RUSTFS_OBJECT_DATA_CACHE_MAX_BYTES=$((64 << 20))
  -e RUSTFS_OBJECT_DATA_CACHE_MAX_ENTRY_BYTES=$((10 << 20)) -e RUSTFS_OBJECT_DATA_CACHE_TTL_SECS=300)
MAT=(-e RUSTFS_OBJECT_DATA_CACHE_MODE=fill_materialize_enabled)
B=qd-verify
V=qd-verify-versioned

echo "== Seed: one node, caches off"
docker volume create "$P-data" >/dev/null
run seed
code -X PUT "$U/$B" >/dev/null
code -X PUT "$U/$V" >/dev/null
code -X PUT "$U/$V?versioning" -d '<VersioningConfiguration xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Status>Enabled</Status></VersioningConfiguration>' >/dev/null
for f in "$D"/o/b* "$D"/o/m*; do code -T "$f" "$U/$B/big/$(basename "$f")" >/dev/null; done
for i in $(seq -w 1 200); do code -T "$D/o/s" "$U/$B/small/s$i" >/dev/null; done
expect "64 MiB read back" "$(fsha "$D/o/b64")" "$(sha "$U/$B/big/b64")"
expect "versioning" Enabled "$(s3 "$U/$V?versioning" | grep -o 'Enabled')"
docker rm -f "$P-seed" >/dev/null

# measure MODE: (b) and (c) on the node now running
measure() {
  local m=$1
  echo "-- $m: median ms of repeated requests (first one warms)"
  s3 -o /dev/null "$U/$B/small/s001"
  s3 -o /dev/null "$U/$B/big/m01"
  printf '   GET 4 KiB %s | 1 MiB %s | 8 MiB %s | 64 KiB of 8 MiB %s | 16 MiB %s | 64 MiB %s | HEAD %s | LIST 200 %s\n' \
    "$(med 30 "$U/$B/small/s001")" "$(med 15 "$U/$B/big/b1")" "$(med 15 "$U/$B/big/m01")" \
    "$(med 15 -r 4194304-4259839 "$U/$B/big/m01")" "$(med 9 "$U/$B/big/b16")" \
    "$(med 5 "$U/$B/big/b64")" "$(med 30 -I "$U/$B/small/s001")" "$(med 15 "$U/$B?list-type=2&prefix=small/")"
  local before
  before=$(mem "$P-$m")
  for _ in 1 2 3; do for f in b1 b4 b16 b64 m01 m02 m03 m04 m05 m06 m07 m08 m09 m10 m11 m12 m13 m14 m15 m16; do
    s3 -o /dev/null "$U/$B/big/$f"
  done; done
  echo "   memory: started $before, after reading 213 MiB three times $(mem "$P-$m")"
  sleep 2
  local st
  st=$(stats "$U")
  [ -n "$st" ] && echo "   admin stats: $(echo "$st" | jq -c . 2>/dev/null | cut -c1-600)"
}

# fresh MODE: (d) on the node now running, with fresh keys under MODE/
fresh() {
  local m=$1 k v1 v2 i bad=0
  head -c $((1 << 20)) /dev/urandom >"$D/v1"
  for i in $(seq 20); do # PUT over a warmed key, then read at once
    k=$U/$B/$m/put
    head -c $((1 << 20)) /dev/urandom >"$D/v2"
    code -T "$D/v1" "$k" >/dev/null
    s3 -o /dev/null "$k" && s3 -o /dev/null "$k"
    code -T "$D/v2" "$k" >/dev/null
    [ "$(sha "$k")" = "$(fsha "$D/v2")" ] || bad=$((bad + 1))
    [ "$(s3 -I "$k" | grep -i '^content-length' | tr -dc 0-9)" = $((1 << 20)) ] || bad=$((bad + 1))
  done
  expect "$m: 20 overwrites of a warmed 1 MiB key read new at once" 0 "$bad"
  code -T "$D/o/s" "$U/$B/$m/copy" >/dev/null && s3 -o /dev/null "$U/$B/$m/copy" && s3 -o /dev/null "$U/$B/$m/copy"
  code -X PUT -H "x-amz-copy-source: /$B/big/b1" "$U/$B/$m/copy" >/dev/null
  s3 -o /dev/null "$U/$B/big/b1" && s3 -o /dev/null "$U/$B/big/b1"
  expect "$m: a range of a warmed 1 MiB object" "$(dd if="$D/o/b1" bs=1 skip=1000 count=5000 2>/dev/null | sha256sum | cut -c1-16)" \
    "$(sha -r 1000-5999 "$U/$B/big/b1")"
  expect "$m: CopyObject over a warmed key" "$(fsha "$D/o/b1")" "$(sha "$U/$B/$m/copy")"
  k=$U/$B/$m/mp
  code -T "$D/o/s" "$k" >/dev/null && s3 -o /dev/null "$k" && s3 -o /dev/null "$k"
  local up etag
  up=$(s3 -X POST "$k?uploads" | grep -o '<UploadId>[^<]*' | cut -d'>' -f2)
  etag=$(s3 -D - -o /dev/null -T "$D/o/b4" "$k?partNumber=1&uploadId=$up" | grep -i '^etag' | cut -d' ' -f2 | tr -d '\r')
  code -X POST "$k?uploadId=$up" -d "<CompleteMultipartUpload><Part><PartNumber>1</PartNumber><ETag>$etag</ETag></Part></CompleteMultipartUpload>" >/dev/null
  expect "$m: multipart upload over a warmed key" "$(fsha "$D/o/b4")" "$(sha "$k")"
  bad=0
  for i in $(seq 10); do
    k=$U/$B/$m/del$i
    code -T "$D/v1" "$k" >/dev/null && s3 -o /dev/null "$k" && s3 -o /dev/null "$k" && code -I "$k" >/dev/null
    code -X DELETE "$k" >/dev/null
    [ "$(code "$k")" = 404 ] && [ "$(code -I "$k")" = 404 ] || bad=$((bad + 1))
  done
  expect "$m: 10 deletes of a warmed key: GET and HEAD 404 at once" 0 "$bad"
  k=$U/$V/$m/obj
  v1=$(s3 -D - -o /dev/null -T "$D/v1" "$k" | grep -i '^x-amz-version-id' | cut -d' ' -f2 | tr -d '\r')
  s3 -o /dev/null "$k" && s3 -o /dev/null "$k"
  v2=$(s3 -D - -o /dev/null -T "$D/o/b1" "$k" | grep -i '^x-amz-version-id' | cut -d' ' -f2 | tr -d '\r')
  expect "$m: versioned, latest after an overwrite" "$(fsha "$D/o/b1")" "$(sha "$k")"
  expect "$m: versioned, the first version by id" "$(fsha "$D/v1")" "$(sha "$k?versionId=$v1")"
  code -X DELETE "$k" >/dev/null
  expect "$m: versioned, after a delete marker" 404 "$(code "$k")"
  expect "$m: versioned, the second version by id after it" "$(fsha "$D/o/b1")" "$(sha "$k?versionId=$v2")"
}

echo "== (a)-(d) one node, $MEM of memory"
for m in off buffered materialize; do
  case $m in off) env=("${META[@]}") ;; buffered) env=("${DATA[@]}") ;; materialize) env=("${DATA[@]}" "${MAT[@]}") ;; esac
  run "$m" "${env[@]}"
  echo "-- $m: what the log says of the caches"
  logged "$P-$m" | sed 's/^/   /'
  measure "$m"
  fresh "$m"
  docker rm -f "$P-$m" >/dev/null
done

for cap in 0 32 96; do
  echo "== (c) the cap: $cap MiB (0: off), $ROUNDS rounds over 100 objects of 1 MiB and 16 of 8 MiB (228 MiB)"
  if [ "$cap" = 0 ]; then run cap; else
    run cap -e RUSTFS_OBJECT_DATA_CACHE_ENABLE=true -e RUSTFS_OBJECT_DATA_CACHE_MAX_BYTES=$((cap << 20)) \
      -e RUSTFS_OBJECT_DATA_CACHE_MAX_ENTRY_BYTES=$((10 << 20)) "${MAT[@]}"
  fi
  [ "$cap" = 0 ] && for i in $(seq -w 1 100); do code -T "$D/o/b1" "$U/$B/one/o$i" >/dev/null; done
  echo "   before the rounds: $(mem "$P-cap")"
  most=0
  for r in $(seq "$ROUNDS"); do
    for i in $(seq -w 1 100); do s3 -o /dev/null "$U/$B/one/o$i"; done
    for i in $(seq -w 1 16); do s3 -o /dev/null "$U/$B/big/m$i"; done
    sleep 2
    e=$(stats "$U" | jq .entries)
    [ "$e" -gt "$most" ] && most=$e
    echo "   round $r: entries $e, $(mem "$P-cap"), $(stats "$U" | jq -c '{hits,fills,memory_pressure_events}')"
  done
  [ "$cap" = 0 ] || expect "entries of 1 MiB never above the $cap MiB cap" yes "$([ "$most" -le "$cap" ] && echo yes || echo "no ($most)")"
  docker rm -f "$P-cap" >/dev/null
done

echo "== (a) the cap as a share of the container's memory, no bytes given"
run pct "${META[@]}" -e RUSTFS_OBJECT_DATA_CACHE_ENABLE=true -e RUSTFS_OBJECT_DATA_CACHE_MAX_MEMORY_PERCENT=10
logged "$P-pct" | sed 's/^/   /'
docker rm -f "$P-pct" >/dev/null
echo "== (a) defaults: the data cache enabled and nothing else"
run dflt -e RUSTFS_OBJECT_DATA_CACHE_ENABLE=true
logged "$P-dflt" | sed 's/^/   /'
echo "   admin stats: $(stats "$U" | jq -c . 2>/dev/null | cut -c1-600)"
docker rm -f "$P-dflt" >/dev/null
echo "== (a) an unknown mode"
run badmode -e RUSTFS_OBJECT_DATA_CACHE_ENABLE=true -e RUSTFS_OBJECT_DATA_CACHE_MODE=bogus
logged "$P-badmode" | sed 's/^/   /'
docker rm -f "$P-badmode" >/dev/null

echo "== (e) a pool of 4, the data cache (materialize) on every node"
docker network create "$P" >/dev/null || exit 1
for i in 1 2 3 4; do
  docker run -d --name "$P-n$i" --network "$P" --memory 512m --env-file "$D/keys.env" -e RUSTFS_OBS_LOGGER_LEVEL=info \
    "${DATA[@]}" "${MAT[@]}" -e "RUSTFS_VOLUMES=http://$P-n{1...4}:9000/data" -v "$P-n$i:/data" \
    -p 127.0.0.1::9000 "$IMAGE" server --address :9000 >/dev/null
done
N1=$(url "$P-n1") N2=$(url "$P-n2")
formed=no
for _ in $(seq 90); do [ "$(code -X PUT "$N1/$B")" = 200 ] && formed=yes && break; sleep 1; done
expect "the pool formed" yes "$formed"
code -X PUT "$N1/$V" >/dev/null
code -X PUT "$N1/$V?versioning" -d '<VersioningConfiguration xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Status>Enabled</Status></VersioningConfiguration>' >/dev/null
bad=0 badd=0
for i in $(seq 20); do
  head -c $((1 << 20)) /dev/urandom >"$D/v1"
  head -c $((1 << 20)) /dev/urandom >"$D/v2"
  code -T "$D/v1" "$N1/$B/x/k" >/dev/null
  s3 -o /dev/null "$N2/$B/x/k" && s3 -o /dev/null "$N2/$B/x/k" && code -I "$N2/$B/x/k" >/dev/null
  code -T "$D/v2" "$N1/$B/x/k" >/dev/null
  [ "$(sha "$N2/$B/x/k")" = "$(fsha "$D/v2")" ] || bad=$((bad + 1))
  code -T "$D/v1" "$N1/$B/x/d$i" >/dev/null
  s3 -o /dev/null "$N2/$B/x/d$i" && s3 -o /dev/null "$N2/$B/x/d$i"
  code -X DELETE "$N1/$B/x/d$i" >/dev/null
  [ "$(code "$N2/$B/x/d$i")" = 404 ] && [ "$(code -I "$N2/$B/x/d$i")" = 404 ] || badd=$((badd + 1))
done
expect "20 overwrites on node 1 of a key warmed on node 2: node 2 reads new at once" 0 "$bad"
expect "20 deletes on node 1 of a key warmed on node 2: node 2 GET and HEAD 404" 0 "$badd"
code -T "$D/v1" "$N1/$V/k" >/dev/null
s3 -o /dev/null "$N2/$V/k" && s3 -o /dev/null "$N2/$V/k"
code -T "$D/v2" "$N1/$V/k" >/dev/null
expect "versioned: node 2 reads the latest after node 1 overwrites" "$(fsha "$D/v2")" "$(sha "$N2/$V/k")"
code -X DELETE "$N1/$V/k" >/dev/null
expect "versioned: node 2 after node 1's delete marker" 404 "$(code "$N2/$V/k")"
echo "   node 2 admin stats: $(stats "$N2" | jq -c . 2>/dev/null | cut -c1-600)"

if [ "$FAILED" = 0 ]; then echo "ALL OK"; else echo "SOME CHECKS FAILED"; fi
exit "$FAILED"
