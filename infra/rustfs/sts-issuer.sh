#!/bin/sh
# The platform's own RustFS issuer (decision 0030): the user the API calls
# STS AssumeRole with, to hand each job credentials narrowed by a session
# policy to its tenant's warehouse. Its own policy (issuer/policy.json) is
# the ceiling: the tenants' warehouses (`qd-tenant-*`) and their tables,
# never the admin API nor another bucket. An installation step, run once
# with RustFS up, with the root credentials of .env (as the installer has
# them); it appends the issuer's keys to .env without printing them
# (RUSTFS_STS_ISSUER_ACCESS_KEY, RUSTFS_STS_ISSUER_SECRET_KEY).
#   infra/rustfs/sts-issuer.sh
set -eu
cd "$(dirname "$0")/../.."
set -a; . ./.env; set +a
container=${RUSTFS_CONTAINER:-data-quality-lake-rustfs-1}
access="qd-sts-issuer-$(openssl rand -hex 4)"
secret=$(openssl rand -hex 24)

# admin METHOD PATH: a request signed with the root credentials (body on stdin).
admin() {
  docker run --rm -i --network "container:$container" curlimages/curl:8.11.1 \
    -s -o /dev/null -w '%{http_code}' -X "$1" \
    --aws-sigv4 "aws:amz:us-east-1:s3" --user "$RUSTFS_ACCESS_KEY:$RUSTFS_SECRET_KEY" \
    -H 'content-type: application/json' --data-binary @- "http://127.0.0.1:9000$2"
}
check() {
  echo "$1: $2"
  case "$2" in 200 | 204) ;; *) exit 1 ;; esac
}
# The ceiling, set again on every run: a new action reaches the issuer
# that is already there (decision 0055 added deleting tables).
check "policy qd-sts-issuer" "$(admin PUT '/rustfs/admin/v3/add-canned-policy?name=qd-sts-issuer' <infra/rustfs/issuer/policy.json)"
if [ -n "${RUSTFS_STS_ISSUER_ACCESS_KEY:-}" ]; then
  echo "sts issuer: already in .env"
  exit 0
fi
check "user" "$(printf '{"secretKey":"%s","status":"enabled"}' "$secret" |
  admin PUT "/rustfs/admin/v3/add-user?accessKey=$access")"
check "user's policy" "$(admin PUT "/rustfs/admin/v3/set-user-or-group-policy?policyName=qd-sts-issuer&userOrGroup=$access&isGroup=false" </dev/null)"
printf '\n# The platform'"'"'s RustFS issuer for per-job credentials (infra/rustfs/sts-issuer.sh)\nRUSTFS_STS_ISSUER_ACCESS_KEY=%s\nRUSTFS_STS_ISSUER_SECRET_KEY=%s\n' \
  "$access" "$secret" >>.env
