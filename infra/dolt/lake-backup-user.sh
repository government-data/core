#!/bin/sh
# The lake's side of the catalog backups: the backup bucket and a RustFS user
# that may only read and write its `dolt/` prefix (no admin, no other bucket).
# Run once per installation, from the repository root, with the lake up:
#   infra/dolt/lake-backup-user.sh
# It generates the user's keys into .env if they are not there
# (DOLT_BACKUP_S3_ACCESS_KEY, DOLT_BACKUP_S3_SECRET_KEY) and calls RustFS's
# S3 and admin APIs signed with the root credentials of .env; idempotent.
# In development RustFS listens on 127.0.0.1:9000 in the network namespace
# of the application's Traefik; elsewhere set RUSTFS_NETWORK and
# DOLT_BACKUP_S3_URL.
set -eu
cd "$(dirname "$0")/../.."
gen() { head -c "$1" /dev/urandom | od -An -tx1 | tr -d ' \n'; }
grep -q '^DOLT_BACKUP_S3_ACCESS_KEY=.' .env || printf 'DOLT_BACKUP_S3_ACCESS_KEY=qd-dolt-backup-%s\n' "$(gen 4)" >> .env
grep -q '^DOLT_BACKUP_S3_SECRET_KEY=.' .env || printf 'DOLT_BACKUP_S3_SECRET_KEY=%s\n' "$(gen 24)" >> .env
set -a; . ./.env; set +a
bucket=${DOLT_BACKUP_BUCKET:-qd-backups}
url=${DOLT_BACKUP_S3_URL:-http://127.0.0.1:9000}
net=${RUSTFS_NETWORK:-container:${APP_STACK:-data-quality}-traefik-1}
policy=qd-dolt-backup

call() {  # method path [body on stdin] -> HTTP code
  docker run --rm -i --network "$net" curlimages/curl:8.11.1 \
    -s -o /dev/null -w '%{http_code}' -X "$1" \
    --aws-sigv4 "aws:amz:us-east-1:s3" --user "$RUSTFS_ACCESS_KEY:$RUSTFS_SECRET_KEY" \
    -H 'content-type: application/json' --data-binary @- "$url$2"
}

# The bucket: 200 created, 409 already there.
code=$(call PUT "/$bucket" </dev/null)
echo "bucket $bucket: $code"
case $code in 200|409) ;; *) exit 1 ;; esac

# Reading the key `dolt` itself lets rclone see a 404, so a directory, not a
# 403 that it takes for a file.
code=$(call PUT "/rustfs/admin/v3/add-canned-policy?name=$policy" <<EOF
{"Version": "2012-10-17", "Statement": [
  {"Effect": "Allow", "Action": ["s3:ListBucket"], "Resource": ["arn:aws:s3:::$bucket"],
   "Condition": {"StringLike": {"s3:prefix": ["dolt/*", "dolt"]}}},
  {"Effect": "Allow", "Action": ["s3:GetBucketLocation"], "Resource": ["arn:aws:s3:::$bucket"]},
  {"Effect": "Allow", "Action": ["s3:GetObject", "s3:PutObject", "s3:DeleteObject"],
   "Resource": ["arn:aws:s3:::$bucket/dolt/*"]},
  {"Effect": "Allow", "Action": ["s3:GetObject"], "Resource": ["arn:aws:s3:::$bucket/dolt"]}
]}
EOF
)
echo "policy $policy: $code"
[ "$code" = 200 ] || exit 1

code=$(printf '{"secretKey": "%s", "status": "enabled"}' "$DOLT_BACKUP_S3_SECRET_KEY" |
  call PUT "/rustfs/admin/v3/add-user?accessKey=$DOLT_BACKUP_S3_ACCESS_KEY")
echo "user: $code"
[ "$code" = 200 ] || exit 1

code=$(call PUT "/rustfs/admin/v3/set-user-or-group-policy?policyName=$policy&userOrGroup=$DOLT_BACKUP_S3_ACCESS_KEY&isGroup=false" </dev/null)
echo "user policy: $code"
[ "$code" = 200 ] || exit 1
