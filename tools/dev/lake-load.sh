#!/bin/sh
# Loads a cut of local Parquet files into the development lake's bronze
# layer (decision 0017), so the rules have data to run against before the
# extraction job exists (decision 0020). Development only: the files stay
# in tmp/ (ignored by git and by Docker), and nothing is printed but counts.
#
#   sh tools/dev/lake-load.sh --cut 2026-10-31 [--dir tmp/datos] [--mappings examples/datasets] [--load DATASET[.TABLE]=FILE]
#
# It writes as its own RustFS user, `qd-loader-dev`, limited to the dev
# warehouse (tools/dev/lake-loader-policy.json): never as root. The first
# time it creates the user with the root credentials of .env, through
# RustFS's admin API, as infra/trino/lake-user.sh does, and appends its keys
# to .env (LAKE_LOADER_ACCESS_KEY, LAKE_LOADER_SECRET_KEY) without printing
# them. Needs uv, and RustFS up.
set -eu
cd "$(dirname "$0")/../.."
set -a; . ./.env; set +a
container=${RUSTFS_CONTAINER:-data-quality-lake-rustfs-1}

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

# The policy every time: the same name, so a change reaches the user.
check "policy qd-dev-loader" "$(admin PUT '/rustfs/admin/v3/add-canned-policy?name=qd-dev-loader' <tools/dev/lake-loader-policy.json)"

if [ -z "${LAKE_LOADER_ACCESS_KEY:-}" ]; then
  LAKE_LOADER_ACCESS_KEY="qd-loader-dev-$(openssl rand -hex 4)"
  LAKE_LOADER_SECRET_KEY=$(openssl rand -hex 24)
  printf '\n# The dev lake loader'"'"'s RustFS user (tools/dev/lake-load.sh)\nLAKE_LOADER_ACCESS_KEY=%s\nLAKE_LOADER_SECRET_KEY=%s\n' \
    "$LAKE_LOADER_ACCESS_KEY" "$LAKE_LOADER_SECRET_KEY" >>.env

  check "user" "$(printf '{"secretKey":"%s","status":"enabled"}' "$LAKE_LOADER_SECRET_KEY" |
    admin PUT "/rustfs/admin/v3/add-user?accessKey=$LAKE_LOADER_ACCESS_KEY")"
  check "user's policy" "$(admin PUT "/rustfs/admin/v3/set-user-or-group-policy?policyName=qd-dev-loader&userOrGroup=$LAKE_LOADER_ACCESS_KEY&isGroup=false" </dev/null)"
fi

export LAKE_LOADER_ACCESS_KEY LAKE_LOADER_SECRET_KEY
exec uv run --quiet tools/dev/lake_load.py "$@"
