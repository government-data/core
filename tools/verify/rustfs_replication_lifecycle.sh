#!/bin/bash
# What the application's replication configuration builds on and was not
# verified before (decision 0066 step 11, the user's rulings 3 to 6 of
# 2026-11-13), on RustFS 1.0.5, every call signed with each lake's platform
# admin keys (qd-platform-admin, made with the root keys as join.sh makes
# it), never root:
#   (a) the replica's bucket made with object lock and a default retention
#       COMPLIANCE of 30 days, read back; made again;
#   (b) the source's lifecycle rule expiring noncurrent versions after 7
#       days, read back, set again; with a replication rule on the bucket
#       both are kept, and replication goes on;
#   (c) what the admin user is answered for a warehouse that does not
#       exist (HEAD), and for ListBuckets;
#   (d) how a second remote target call and list-remote-targets answer, to
#       find the target again.
# The expiry itself (7 days) is not waited for.
#
# Two throwaway containers (qd-verify-repllife-…) over plain HTTP on a
# network of their own (TLS and the gate are rustfs_replication_gate.sh's
# and rustfs_source_proxy.sh's), random root keys and secrets never
# printed, synthetic objects. Run from the repository root:
#
#   bash tools/verify/rustfs_replication_lifecycle.sh
#
# Needs docker and uv (boto3, requests).
set -u
export LC_ALL=C
IMAGE=${RUSTFS_IMAGE:-rustfs/rustfs:1.0.5}
P=qd-verify-repllife
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
D=$D SOURCE_IP=$(ip source) REPLICA_IP=$(ip replica) \
  uv run -q --no-project --with boto3==1.44.2 --with requests==2.34.2 \
  python3 -I tools/verify/rustfs_replication_lifecycle.py
