#!/bin/sh
# Create or update in RustFS the storage side of the managed policies
# (ADR 0016): same names as in Quality Data (crates/iam/src/managed.rs), so
# whoever signs in to the console with Quality Data gets the policies of their
# `policy` claim. Until the application syncs them itself (decision 0028),
# run it by hand from anywhere, with RustFS up:
#   infra/rustfs/apply-policies.sh
# It calls RustFS's admin API from its own network (port 9000 is not
# published), signed with the root credentials of .env; each policy goes on
# stdin.
set -eu
cd "$(dirname "$0")/../.."
set -a; . ./.env; set +a
container=${RUSTFS_CONTAINER:-data-quality-lake-rustfs-1}
for file in infra/rustfs/policies/*.json; do
  name=$(basename "$file" .json)
  code=$(docker run --rm -i --network "container:$container" curlimages/curl:8.11.1 \
    -s -o /dev/null -w '%{http_code}' -X PUT \
    --aws-sigv4 "aws:amz:us-east-1:s3" --user "$RUSTFS_ACCESS_KEY:$RUSTFS_SECRET_KEY" \
    -H 'content-type: application/json' --data-binary @- \
    "http://127.0.0.1:9000/rustfs/admin/v3/add-canned-policy?name=$name" <"$file")
  echo "$name: $code"
  [ "$code" = 200 ] || exit 1
done
