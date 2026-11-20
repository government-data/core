#!/bin/sh
# The observability stack's storage in the lake: Loki's (decision 0026) and
# Mimir's (decision 0071), each a bucket and a RustFS user of its own whose
# policy reaches only that bucket. Run once with the lake up, before
# `task obs:up`; running it again changes nothing:
#   infra/observability/setup-storage.sh
# It calls RustFS from its own network (port 9000 is not published), signed
# with the root credentials of .env, as the installation does once. The first
# time it makes each user's keys and appends them to .env
# (LOKI_S3_ACCESS_KEY, LOKI_S3_SECRET_KEY, MIMIR_S3_ACCESS_KEY,
# MIMIR_S3_SECRET_KEY), never shown.
set -eu
cd "$(dirname "$0")/../.."
set -a; . ./.env; set +a
container=${RUSTFS_CONTAINER:-data-quality-lake-rustfs-1}

# curl from the lake's network namespace, signed as root; the body on stdin.
s3() {
  docker run --rm -i --network "container:$container" curlimages/curl:8.11.1 \
    -s -o /dev/null -w '%{http_code}' -X "$1" \
    --aws-sigv4 "aws:amz:us-east-1:s3" --user "$RUSTFS_ACCESS_KEY:$RUSTFS_SECRET_KEY" \
    -H 'content-type: application/json' --data-binary @- "http://127.0.0.1:9000$2"
}
check() {
  echo "$1: $2"
  case "$2" in 200|204) ;; *) exit 1 ;; esac
}

# store SERVICE PREFIX BUCKET USER: the bucket, a policy reaching only it,
# and the user with that policy, its keys in .env as <PREFIX>_S3_ACCESS_KEY
# and <PREFIX>_S3_SECRET_KEY (made the first time).
store() {
  service=$1 prefix=$2 bucket=$3 user=$4
  policy=$user
  eval "access=\${${prefix}_S3_ACCESS_KEY:-} secret=\${${prefix}_S3_SECRET_KEY:-}"
  if [ -z "$access" ] || [ -z "$secret" ]; then
    access=$user
    secret=$(head -c 30 /dev/urandom | od -An -tx1 | tr -d ' \n')
    printf '\n# %s in the lake (infra/observability/setup-storage.sh)\n%s_S3_ACCESS_KEY=%s\n%s_S3_SECRET_KEY=%s\n' \
      "$service" "$prefix" "$access" "$prefix" "$secret" >>.env
    echo "$service's keys: appended to .env"
  fi

  # The bucket (409: it is already there, owned by root).
  code=$(s3 PUT "/$bucket" </dev/null)
  [ "$code" = 409 ] && code=200
  check "bucket $bucket" "$code"

  check "policy $policy" "$(s3 PUT "/rustfs/admin/v3/add-canned-policy?name=$policy" <<EOF
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Effect": "Allow",
      "Action": ["s3:GetBucketLocation", "s3:ListBucket", "s3:ListBucketMultipartUploads"],
      "Resource": ["arn:aws:s3:::$bucket"]
    },
    {
      "Effect": "Allow",
      "Action": ["s3:GetObject", "s3:PutObject", "s3:DeleteObject",
                 "s3:AbortMultipartUpload", "s3:ListMultipartUploadParts"],
      "Resource": ["arn:aws:s3:::$bucket/*"]
    }
  ]
}
EOF
)"

  check "user $access" "$(printf '{"secretKey":"%s","status":"enabled"}' "$secret" |
    s3 PUT "/rustfs/admin/v3/add-user?accessKey=$access")"

  check "user's policy" "$(s3 PUT "/rustfs/admin/v3/set-user-or-group-policy?policyName=$policy&userOrGroup=$access&isGroup=false" </dev/null)"
}

store Loki LOKI "${LOKI_S3_BUCKET:-qd-observability}" qd-observability-loki
store Mimir MIMIR "${MIMIR_S3_BUCKET:-qd-metrics}" qd-metrics-mimir
