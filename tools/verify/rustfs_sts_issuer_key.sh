#!/bin/bash
# Whether RustFS takes the STS issuer's keys as the update agent makes them
# (decision 0091, point 4; module.yaml's `secrets.generate`): the access key
# is 16 random bytes in base64url without padding, 22 characters that may
# hold `-` and `_`; the secret key 32 bytes, 43 characters. MinIO, whose
# admin API RustFS follows, limits an access key to 20 characters, so the
# packaged keys may be refused where the dev keys (infra/rustfs/sts-issuer.sh,
# `qd-sts-issuer-<8 hex>`, 22 characters of [a-z0-9-]) were not. Cases:
#   (k20) a 20-character access key of [a-z0-9]: the control;
#   (k22) a 22-character base64url access key with both `-` and `_`;
#   (k22s) the same length with the secret key of 43 base64url characters.
# Each: the admin API's add-user answers 200, the user gets the issuer's
# policy, and STS AssumeRole signed with the user's keys answers 200 with
# temporary credentials that list the bucket.
#
# Throwaway containers only (qd-verify-stskey-…, a network of their own, no
# port published), random root keys never printed, no data. Run alone (no
# build at the same time), from the repository root:
#
#   bash tools/verify/rustfs_sts_issuer_key.sh
#
# Needs docker, curl 7.75 or later (--aws-sigv4) and openssl.
set -u
export LC_ALL=C
IMAGE=${RUSTFS_IMAGE:-rustfs/rustfs:1.0.5}
docker image inspect "$IMAGE" -f 'image: {{index .RepoDigests 0}}' 2>/dev/null || true
P=qd-verify-stskey
WAIT=${QD_VERIFY_WAIT:-60}
D=$(mktemp -d)
chmod 700 "$D"
cleanup() {
  docker ps -aq --filter "name=^$P-" | xargs -r docker rm -f >/dev/null 2>&1
  docker network rm "$P" >/dev/null 2>&1
  docker run --rm -v "$D:/d" alpine:3.20 sh -c 'rm -rf /d/drive' >/dev/null 2>&1
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
ROOT_ACCESS="qd$(openssl rand -hex 8)"
ROOT_SECRET=$(openssl rand -hex 20)
printf 'RUSTFS_ACCESS_KEY=%s\nRUSTFS_SECRET_KEY=%s\n' "$ROOT_ACCESS" "$ROOT_SECRET" >"$D/root.env"
chmod 600 "$D/root.env"
# cfg FILE ACCESS SECRET SERVICE: a curl config, so no key is on a command line.
cfg() {
  printf 'user = "%s:%s"\naws-sigv4 = "aws:amz:us-east-1:%s"\n' "$2" "$3" "$4" >"$1"
  chmod 600 "$1"
}
cfg "$D/root.cfg" "$ROOT_ACCESS" "$ROOT_SECRET" s3
mkdir -p "$D/drive"
docker run --rm -v "$D/drive:/x" alpine:3.20 chown 10001:10001 /x >/dev/null
N="$P-node"
docker run -d --name "$N" --network "$P" --env-file "$D/root.env" -e RUSTFS_VOLUMES=/data \
  -v "$D/drive:/data" "$IMAGE" server --address :9000 >/dev/null || exit 1
IP=$(docker inspect -f "{{(index .NetworkSettings.Networks \"$P\").IPAddress}}" "$N")
URL="http://$IP:9000"
t=0
until [ "$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 -K "$D/root.cfg" "$URL/")" = 200 ]; do
  t=$((t + 2))
  [ $t -lt "$WAIT" ] || { ko "RustFS not ready after ${WAIT}s"; exit 1; }
  sleep 2
done
ok "RustFS answers"

# admin METHOD PATH [FILE]: a request signed with the root keys.
admin() {
  local f=(--data-binary "")
  [ -z "${3:-}" ] || f=(--data-binary "@$3")
  curl -s -o "$D/admin.out" -w '%{http_code}' --max-time 15 -K "$D/root.cfg" -X "$1" \
    -H 'content-type: application/json' "${f[@]}" "$URL$2"
}
printf '%s' '{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:*"],"Resource":["arn:aws:s3:::qd-verify*"]}]}' >"$D/policy.json"
[ "$(admin PUT '/rustfs/admin/v3/add-canned-policy?name=qd-verify-issuer' "$D/policy.json")" = 200 ] &&
  ok "the issuer's policy" || ko "the issuer's policy: $(cat "$D/admin.out")"
[ "$(curl -s -o /dev/null -w '%{http_code}' -K "$D/root.cfg" -X PUT "$URL/qd-verify")" = 200 ] &&
  ok "a bucket" || ko "a bucket"

b64url() { openssl rand -base64 "$1" | tr '+/' '-_' | tr -d '=\n'; }
# A 16-byte base64url key that holds both `-` and `_` (drawn again until so).
key22() {
  local k
  while :; do
    k=$(b64url 16)
    case "$k" in *-*_* | *_*-*) echo "$k"; return ;; esac
  done
}

# case TAG ACCESS SECRET
try() {
  local tag=$1 access=$2 secret=$3
  printf '{"secretKey":"%s","status":"enabled"}' "$secret" >"$D/user.json"
  chmod 600 "$D/user.json"
  local add
  add=$(admin PUT "/rustfs/admin/v3/add-user?accessKey=$access" "$D/user.json")
  if [ "$add" != 200 ]; then
    ko "($tag) ${#access}-character access key refused at add-user ($add): $(head -c 300 "$D/admin.out")"
    return
  fi
  ok "($tag) ${#access}-character access key taken at add-user"
  local pol
  pol=$(admin PUT "/rustfs/admin/v3/set-user-or-group-policy?policyName=qd-verify-issuer&userOrGroup=$access&isGroup=false")
  [ "$pol" = 200 ] && ok "($tag) the user's policy" || ko "($tag) the user's policy ($pol)"
  cfg "$D/user.cfg" "$access" "$secret" sts
  local sts
  sts=$(curl -s -o "$D/sts.out" -w '%{http_code}' --max-time 15 -K "$D/user.cfg" -X POST \
    -H 'content-type: application/x-www-form-urlencoded' \
    --data 'Action=AssumeRole&Version=2011-06-15&DurationSeconds=900' "$URL/")
  local tmp_access tmp_secret tmp_token
  tmp_access=$(sed -n 's:.*<AccessKeyId>\([^<]*\)</AccessKeyId>.*:\1:p' "$D/sts.out")
  tmp_secret=$(sed -n 's:.*<SecretAccessKey>\([^<]*\)</SecretAccessKey>.*:\1:p' "$D/sts.out")
  tmp_token=$(sed -n 's:.*<SessionToken>\([^<]*\)</SessionToken>.*:\1:p' "$D/sts.out")
  if [ "$sts" != 200 ] || [ -z "$tmp_access" ]; then
    ko "($tag) AssumeRole ($sts): $(sed 's/<SecretAccessKey>[^<]*/<SecretAccessKey>…/; s/<SessionToken>[^<]*/<SessionToken>…/' "$D/sts.out" | head -c 300)"
    return
  fi
  ok "($tag) AssumeRole gives temporary credentials"
  cfg "$D/tmp.cfg" "$tmp_access" "$tmp_secret" s3
  printf 'header = "x-amz-security-token: %s"\n' "$tmp_token" >>"$D/tmp.cfg"
  local list
  list=$(curl -s -o /dev/null -w '%{http_code}' --max-time 15 -K "$D/tmp.cfg" "$URL/qd-verify?list-type=2")
  [ "$list" = 200 ] && ok "($tag) they list the bucket" || ko "($tag) they list the bucket ($list)"
}

try k20 "qd$(openssl rand -hex 9)" "$(openssl rand -hex 20)"
try k22 "$(key22)" "$(openssl rand -hex 20)"
try k22s "$(key22)" "$(b64url 32)"

echo
if [ "$FAILED" = 0 ]; then echo "ALL OK"; else echo "SOME CHECKS FAILED"; fi
exit "$FAILED"
