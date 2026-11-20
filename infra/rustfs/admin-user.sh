#!/bin/sh
# The platform's admin user on the main lake (decision 0028, point 2; 0066
# step 11, the user's ruling 1 of 2026-11-13): run when the lake's stack is
# brought up (`task lake:up` runs it after `up -d`), on the lake's server,
# from the repository's directory:
#
#   sh infra/rustfs/admin-user.sh
#
# It does for today's single RustFS what infra/join/join.sh does for the
# replica (its step 6), with the same user and policy (a test keeps the two
# in step): once RustFS answers, it sets the policy qd-platform-admin
# (every action on the tenants' warehouses and the admin API), asks whether
# the user exists and only if not makes it, its secret 64 hex digits from
# the system's CSPRNG, and shows its keys ONCE, on this terminal alone, to
# be pasted in Settings → Lake under the main lake; then attaches the
# policy. A user already there is not made again and no secret is shown:
# RustFS's add-user would replace its secret.
#
# The calls are made by RustFS's own curl inside its container: it reads
# the root keys from that container's environment and everything else
# from its standard input (its config, -K -), so no key is in a program's
# arguments, and an exec's output is not in the container's log. With the
# machines' certificates (infra/rustfs/certificate.yml) RustFS is reached
# over TLS by the node's name (QD_MACHINE_NAME, from the environment or
# .env), checked against the agent's CA; otherwise (development) over
# plain HTTP on its loopback. Nothing it is given or makes is written to
# a file or a log.
#
# Environment: RUSTFS_CONTAINER (data-quality-lake-rustfs-1),
# QD_LAKE_WAIT (360, seconds to wait for RustFS to answer).
set -eu
cd "$(dirname "$0")/../.."

ADMIN_USER=qd-platform-admin
ADMIN_POLICY='{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:*"],"Resource":["arn:aws:s3:::qd-tenant-*","arn:aws:s3:::qd-tenant-*/*"]},{"Effect":"Allow","Action":["admin:*"],"Resource":["arn:aws:s3:::*"]}]}'
CONTAINER=${RUSTFS_CONTAINER:-data-quality-lake-rustfs-1}
LAKE_WAIT=${QD_LAKE_WAIT:-360}

say() { printf 'qd-lake: %s\n' "$*"; }
die() {
  printf 'qd-lake: %s\n' "$*" >&2
  exit 1
}

command -v docker >/dev/null || die "docker is needed"
command -v od >/dev/null || die "od is needed"
docker inspect "$CONTAINER" >/dev/null 2>&1 || die "no container $CONTAINER: bring the lake's stack up first"

# Where RustFS answers inside its own container.
# shellcheck disable=SC2016 # expanded inside the container
tls=$(docker exec "$CONTAINER" sh -c 'printf %s "${RUSTFS_TLS_PATH:-}"')
if [ -n "$tls" ]; then
  name=${QD_MACHINE_NAME:-}
  [ -n "$name" ] || [ ! -f .env ] || name=$(sed -n 's/^QD_MACHINE_NAME=//p' .env | tail -n 1)
  printf '%s' "$name" | grep -Eq '^[A-Za-z0-9.-]+$' ||
    die "RustFS serves TLS: set QD_MACHINE_NAME to the lake node's name, as in its certificate"
  BASE="https://$name:9000"
  REACH=$(printf 'resolve = "%s:9000:127.0.0.1"\ncacert = "/run/qd-node/ca.crt"\n' "$name")
else
  BASE="http://127.0.0.1:9000"
  REACH=''
fi

# s3 METHOD PATH [BODY]: the HTTP status of METHOD on RustFS's PATH.
s3() {
  {
    printf 'variable = "%%RUSTFS_ACCESS_KEY"\nvariable = "%%RUSTFS_SECRET_KEY"\n'
    printf 'expand-user = "{{RUSTFS_ACCESS_KEY}}:{{RUSTFS_SECRET_KEY}}"\naws-sigv4 = "aws:amz:us-east-1:s3"\n'
    [ -z "$REACH" ] || printf '%s\n' "$REACH"
    printf 'silent\nmax-time = 20\noutput = "/dev/null"\nwrite-out = "%%{http_code}"\n'
    printf 'request = "%s"\nurl = "%s%s"\n' "$1" "$BASE" "$2"
    [ $# -lt 3 ] || printf 'data-binary = "%s"\n' "$(printf '%s' "$3" | sed 's/\\/\\\\/g; s/"/\\"/g')"
  } | docker exec -i "$CONTAINER" curl -K - 2>/dev/null
}

AGAIN="run this script again once RustFS answers"
waited=0
until [ "$(s3 GET /)" = 200 ]; do
  [ "$waited" -lt "$LAKE_WAIT" ] ||
    die "RustFS did not answer within ${LAKE_WAIT}s, so the platform's admin user was not made: $AGAIN"
  sleep 5
  waited=$((waited + 5))
done
[ "$(s3 PUT "/rustfs/admin/v3/add-canned-policy?name=$ADMIN_USER" "$ADMIN_POLICY")" = 200 ] ||
  die "RustFS refused the platform's admin policy: $AGAIN"
case $(s3 GET "/rustfs/admin/v3/user-info?accessKey=$ADMIN_USER") in
200) say "the platform's admin user $ADMIN_USER is there already: its secret was shown when it was made and is not shown again" ;;
404)
  # RustFS's add-user replaces the secret of a user that exists, so it is
  # only asked for when user-info says there is none.
  secret=$(od -An -tx1 -N32 /dev/urandom | tr -d ' \n')
  printf '%s' "$secret" | grep -Eq '^[0-9a-f]{64}$' || die "no secret could be made from /dev/urandom"
  [ "$(s3 PUT "/rustfs/admin/v3/add-user?accessKey=$ADMIN_USER" "{\"secretKey\":\"$secret\",\"status\":\"enabled\"}")" = 200 ] ||
    die "RustFS refused the platform's admin user: $AGAIN"
  printf '\n%s\n\n  access key: %s\n  secret key: %s\n\n%s\n%s\n\n' \
    "qd-lake: the platform's admin user of the main lake, SHOWN ONLY THIS ONCE:" "$ADMIN_USER" "$secret" \
    "qd-lake: paste both now in Quality Data, Settings → Lake, The main lake. They are written to" \
    "qd-lake: no file and cannot be shown again; this terminal is the only place they are."
  secret=''
  ;;
*) die "RustFS did not say whether the platform's admin user exists: $AGAIN" ;;
esac
[ "$(s3 PUT "/rustfs/admin/v3/set-user-or-group-policy?policyName=$ADMIN_USER&userOrGroup=$ADMIN_USER&isGroup=false")" = 200 ] ||
  die "RustFS did not give the platform's admin user its policy: $AGAIN"
say "the platform's admin user has its policy"
