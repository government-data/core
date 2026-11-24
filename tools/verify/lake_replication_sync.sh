#!/bin/bash
# What RustFS 1.0.5 reports of a bucket's replication, to say whether a
# lake's replica is in sync (decision 0078, the amendment "the agent asks
# whether the lake's replica is in sync"): per bucket,
# `GET /rustfs/admin/v3/replicationmetrics?bucket=` (its fields, what waits
# to be copied and what failed) and the bucket's replication configuration;
# per object, `x-amz-replication-status`. Three moments:
#   (a) configured, objects written and copied;
#   (b) the replica stopped, objects written: what waits, what failed;
#   (c) the replica back: whether what waited arrives by itself, and what
#       the counts say once it has (a failure counted for good, or only for
#       a while).
# Every call is signed with each lake's platform admin keys (made with the
# root keys as join.sh makes them), never root.
#
# Two throwaway containers (qd-verify-replsync-…) over plain HTTP on a
# network of their own, removed in a trap with their volumes and network;
# random keys never printed; synthetic objects. Run alone (no build at the
# same time), from the repository root:
#
#   bash tools/verify/lake_replication_sync.sh
#
# Needs docker and uv (boto3, requests).
set -u
export LC_ALL=C
IMAGE=${RUSTFS_IMAGE:-rustfs/rustfs:1.0.5}
P=qd-verify-replsync
D=$(mktemp -d)
chmod 700 "$D"
cleanup() {
  docker ps -aq --filter "name=^$P-" | xargs -r docker rm -f -v >/dev/null 2>&1
  docker network rm "$P" >/dev/null 2>&1
  rm -rf "$D"
}
trap cleanup EXIT
docker network create "$P" >/dev/null || exit 1
for n in source replica; do
  printf 'RUSTFS_ACCESS_KEY=qd%s\nRUSTFS_SECRET_KEY=%s\n' "$(openssl rand -hex 8)" "$(openssl rand -hex 20)" >"$D/root-$n.env"
  docker run -d --name "$P-$n" --network "$P" --network-alias "$n" --env-file "$D/root-$n.env" \
    -e RUSTFS_VOLUMES=/data "$IMAGE" server --address :9000 >/dev/null || exit 1
done
sleep 6
POLICY=$(sed -n "s/^ADMIN_POLICY='\(.*\)'$/\1/p" infra/join/join.sh)
admin_user() { # NAME: qd-platform-admin made with the root keys, its keys in admin-NAME.env
  local secret code all=""
  secret=$(od -An -tx1 -N32 /dev/urandom | tr -d ' \n')
  printf 'ADMIN_ACCESS_KEY=qd-platform-admin\nADMIN_SECRET_KEY=%s\n' "$secret" >"$D/admin-$1.env"
  for call in "PUT /rustfs/admin/v3/add-canned-policy?name=qd-platform-admin $POLICY" \
    "PUT /rustfs/admin/v3/add-user?accessKey=qd-platform-admin {\"secretKey\":\"$secret\",\"status\":\"enabled\"}" \
    "PUT /rustfs/admin/v3/set-user-or-group-policy?policyName=qd-platform-admin&userOrGroup=qd-platform-admin&isGroup=false "; do
    set -- "$1" $call
    code=$(printf 'url = "http://127.0.0.1:9000%s"\nrequest = "%s"\ndata-binary = "%s"\n' "$3" "$2" "$(printf '%s' "${4:-}" | sed 's/"/\\"/g')" |
      docker exec -i "$P-$1" curl -sS -o /dev/null -w '%{http_code}' --variable %RUSTFS_ACCESS_KEY \
        --variable %RUSTFS_SECRET_KEY --expand-user '{{RUSTFS_ACCESS_KEY}}:{{RUSTFS_SECRET_KEY}}' \
        --aws-sigv4 aws:amz:us-east-1:s3 -K -)
    all="$all $code"
  done
  echo "$1's admin user:$all"
}
admin_user source
admin_user replica
ip() { docker inspect -f "{{(index .NetworkSettings.Networks \"$P\").IPAddress}}" "$P-$1"; }
SOURCE_IP=$(ip source)
REPLICA_IP=$(ip replica)
py() {
  D=$D SOURCE_IP=$SOURCE_IP REPLICA_IP=$REPLICA_IP MODE=$1 \
    uv run -q --no-project --with boto3==1.44.2 --with requests==2.34.2 \
    python3 -I tools/verify/lake_replication_sync.py
}
FAILED=0
py configured || FAILED=1
docker stop -t 2 "$P-replica" >/dev/null
py replica_down || FAILED=1
docker start "$P-replica" >/dev/null
sleep 6
py replica_back || FAILED=1
echo "run $(date -u +%Y-%m-%dT%H:%MZ) with $IMAGE ($(docker image inspect -f '{{.Id}}' "$IMAGE" | cut -c1-19))"
exit $FAILED
