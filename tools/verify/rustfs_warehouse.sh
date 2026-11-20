#!/bin/bash
# What the application's "ensure a tenant's warehouse" step builds on
# (decision 0066 step 11, the user's ruling of 2026-11-13 that the
# application makes the warehouse; 0028's "a tenant's warehouse"), on
# RustFS 1.0.5, every call signed with the main lake's platform admin keys
# (qd-platform-admin, made with the root keys as admin-user.sh makes it),
# never root:
#   (a) a missing warehouse: HEAD answers 404; the table bucket of a
#       bucket that does not exist;
#   (b) the warehouse made: the bucket, then its Iceberg table bucket, read
#       back; a namespace and a table made through the catalog;
#   (c) a second run: HEAD, the bucket made again, the table bucket again;
#       the namespace and table still there;
#   (d) with versioning on (as the replication sets it): the table bucket
#       of a bucket versioned first, and the catalog on it;
#   (e) a bucket outside qd-tenant-* refused.
#
# One throwaway container (qd-verify-warehouse-…) over plain HTTP on a
# network of its own, random root keys and secrets never printed, synthetic
# names. Run from the repository root:
#
#   bash tools/verify/rustfs_warehouse.sh
#
# Needs docker and uv (boto3, requests).
set -u
export LC_ALL=C
IMAGE=${RUSTFS_IMAGE:-rustfs/rustfs:1.0.5}
P=qd-verify-warehouse
D=$(mktemp -d)
chmod 700 "$D"
cleanup() {
  docker ps -aq --filter "name=^$P-" | xargs -r docker rm -f -v >/dev/null 2>&1
  docker network rm "$P" >/dev/null 2>&1
  rm -rf "$D"
}
trap cleanup EXIT
docker network create "$P" >/dev/null || exit 1
printf 'RUSTFS_ACCESS_KEY=qd%s\nRUSTFS_SECRET_KEY=%s\n' "$(openssl rand -hex 8)" "$(openssl rand -hex 20)" >"$D/root.env"
docker run -d --name "$P-lake" --network "$P" --env-file "$D/root.env" \
  -e RUSTFS_VOLUMES=/data "$IMAGE" server --address :9000 >/dev/null || exit 1
sleep 6
POLICY=$(sed -n "s/^ADMIN_POLICY='\(.*\)'$/\1/p" infra/rustfs/admin-user.sh)
[ -n "$POLICY" ] || { echo "no ADMIN_POLICY in admin-user.sh"; exit 1; }
secret=$(od -An -tx1 -N32 /dev/urandom | tr -d ' \n')
printf 'ADMIN_ACCESS_KEY=qd-platform-admin\nADMIN_SECRET_KEY=%s\n' "$secret" >"$D/admin.env"
all=""
for call in "PUT /rustfs/admin/v3/add-canned-policy?name=qd-platform-admin $POLICY" \
  "PUT /rustfs/admin/v3/add-user?accessKey=qd-platform-admin {\"secretKey\":\"$secret\",\"status\":\"enabled\"}" \
  "PUT /rustfs/admin/v3/set-user-or-group-policy?policyName=qd-platform-admin&userOrGroup=qd-platform-admin&isGroup=false "; do
  set -- $call
  code=$(printf 'url = "http://127.0.0.1:9000%s"\nrequest = "%s"\ndata-binary = "%s"\n' "$2" "$1" "$(printf '%s' "${3:-}" | sed 's/"/\\"/g')" |
    docker exec -i "$P-lake" curl -sS -o /dev/null -w '%{http_code}' --variable %RUSTFS_ACCESS_KEY \
      --variable %RUSTFS_SECRET_KEY --expand-user '{{RUSTFS_ACCESS_KEY}}:{{RUSTFS_SECRET_KEY}}' \
      --aws-sigv4 aws:amz:us-east-1:s3 -K -)
  all="$all $code"
done
unset secret
echo "the admin user:$all"
LAKE_IP=$(docker inspect -f "{{(index .NetworkSettings.Networks \"$P\").IPAddress}}" "$P-lake")
D=$D LAKE_IP=$LAKE_IP \
  uv run -q --no-project --with boto3==1.44.2 --with requests==2.34.2 \
  python3 -I tools/verify/rustfs_warehouse.py
