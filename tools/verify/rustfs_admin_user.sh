#!/bin/bash
# How the platform's admin user (decision 0028, point 2) is made on a new
# RustFS with its root keys, over TLS, from the server it runs on, without
# the root keys or the new secret reaching a log or a program's arguments
# (decision 0066, step 11, the user's ruling of 2026-11-13: join.sh makes it
# after starting the replica and prints its keys once on the terminal).
#
#   (a) a client inside RustFS's own container (`docker exec -i`, its curl
#       with --aws-sigv4), the root keys read from that container's
#       environment by curl's --variable, everything else given on curl's
#       standard input as its config (-K -): nothing in any argv;
#   (b) the admin API's calls 0028 verified (add-canned-policy, add-user,
#       set-user-or-group-policy), signed by curl alone; the secret's form
#       (64 hex digits) and the lengths RustFS takes;
#   (c) what the user can then do, signed with its own keys only: the admin
#       API (info, a pool's policy and user, user-info), a tenant's bucket
#       and its table bucket, and nothing outside qd-tenant-*;
#   (d) a second run: whether the user exists is asked first (user-info),
#       and what add-user does to an existing user's secret;
#   (e) what `docker logs` keeps: nothing of an exec's output, nor the
#       secret in RustFS's own log.
#
# A throwaway RustFS (qd-verify-adminuser-rustfs, no port published), a
# throwaway CA, random root keys; none of the keys or secrets is printed.
# Run from the repository root:
#
#   bash tools/verify/rustfs_admin_user.sh
#
# Needs docker and openssl, and the image rustfs/rustfs:1.0.5.
set -u
export LC_ALL=C
IMAGE=${RUSTFS_IMAGE:-rustfs/rustfs:1.0.5}
C=qd-verify-adminuser-rustfs
NAME=replica-1
D=$(mktemp -d)
chmod 700 "$D"
cleanup() {
  docker rm -f -v "$C" >/dev/null 2>&1
  rm -rf "$D"
}
trap cleanup EXIT
FAILED=0
ok() { printf '[OK  ] %s\n' "$*"; }
ko() {
  printf '[FAIL] %s\n' "$*"
  FAILED=1
}
expect() { if [ "$2" = "$3" ]; then ok "$1: $3"; else ko "$1: expected '$2', got '$3'"; fi; }

echo "== A RustFS over TLS, its certificate of a throwaway CA for $NAME"
mkdir -m 755 "$D/tls"
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 -subj "/CN=qd verify admin user" \
  -addext "basicConstraints=critical,CA:TRUE" -keyout "$D/ca.key" -out "$D/tls/ca.crt" 2>/dev/null
openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "/CN=$NAME" \
  -keyout "$D/tls/rustfs_key.pem" -out "$D/node.csr" 2>/dev/null
printf 'subjectAltName=DNS:%s\nextendedKeyUsage=serverAuth,clientAuth\n' "$NAME" >"$D/ext"
openssl x509 -req -in "$D/node.csr" -CA "$D/tls/ca.crt" -CAkey "$D/ca.key" -CAcreateserial -days 1 \
  -extfile "$D/ext" -out "$D/tls/rustfs_cert.pem" 2>/dev/null
cp "$D/tls/ca.crt" "$D/tls/client_ca.crt"
chmod 644 "$D/tls"/*
umask 077
printf 'RUSTFS_ACCESS_KEY=qd%s\nRUSTFS_SECRET_KEY=%s\n' "$(openssl rand -hex 8)" "$(openssl rand -hex 20)" >"$D/root-keys.env"
docker rm -f -v "$C" >/dev/null 2>&1
docker run -d --name "$C" --env-file "$D/root-keys.env" -e RUSTFS_VOLUMES=/data -e RUSTFS_TLS_PATH=/run/rustfs-tls \
  -e RUSTFS_OBS_LOGGER_LEVEL=info -v "$D/tls:/run/rustfs-tls:ro" --tmpfs /data:uid=10001,gid=10001 \
  "$IMAGE" server --address :9000 >/dev/null || { echo "could not start RustFS"; exit 1; }

# as WHO: curl's config lines that sign as WHO. `root` reads the root keys
# from RustFS's environment (--variable %VAR, curl 8.3 or later); a user
# file holds ACCESS SECRET, given on the standard input like the rest.
as() {
  if [ "$1" = root ]; then
    printf 'variable = "%%RUSTFS_ACCESS_KEY"\nvariable = "%%RUSTFS_SECRET_KEY"\n'
    printf 'expand-user = "{{RUSTFS_ACCESS_KEY}}:{{RUSTFS_SECRET_KEY}}"\n'
  else
    read -r ak sk <"$D/$1"
    printf 'user = "%s:%s"\n' "$ak" "$sk"
  fi
  printf 'aws-sigv4 = "aws:amz:us-east-1:s3"\nresolve = "%s:9000:127.0.0.1"\ncacert = "/run/rustfs-tls/ca.crt"\n' "$NAME"
  printf 'silent\nmax-time = 10\noutput = "/dev/null"\nwrite-out = "%%{http_code}"\n'
}
# call WHO METHOD PATH [BODY]: the status of METHOD https://NAME:9000PATH,
# run by curl inside RustFS's container, its whole config on stdin.
call() {
  {
    as "$1"
    printf 'request = "%s"\nurl = "https://%s:9000%s"\n' "$2" "$NAME" "$3"
    [ $# -lt 4 ] || printf 'data-binary = "%s"\n' "$(printf '%s' "$4" | sed 's/\\/\\\\/g; s/"/\\"/g')"
  } | docker exec -i "$C" curl -K -
}

# RustFS answers 503 for a while after its port opens.
for _ in $(seq 90); do [ "$(call root GET /)" = 200 ] && break; sleep 1; done

echo "== (a) the client: RustFS's own curl, nothing in its arguments"
expect "the image's curl signs (--aws-sigv4) and reads variables (8.3 or later)" "yes" \
  "$(docker exec "$C" curl --version | head -1 | awk '{split($2,v,"."); print (v[1]>8 || (v[1]==8 && v[2]>=3)) ? "yes" : "no"}')"
expect "the root keys, from RustFS's environment, list the buckets" 200 "$(call root GET /)"
expect "a wrong key is refused" 403 "$(printf 'user = "nobody:wrongwrongwrong"\naws-sigv4 = "aws:amz:us-east-1:s3"\nresolve = "%s:9000:127.0.0.1"\ncacert = "/run/rustfs-tls/ca.crt"\nsilent\noutput = "/dev/null"\nwrite-out = "%%{http_code}"\nurl = "https://%s:9000/"\n' "$NAME" "$NAME" | docker exec -i "$C" curl -K -)"

echo "== (b) the platform's policy and user, by the root keys"
POLICY='{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:*"],"Resource":["arn:aws:s3:::qd-tenant-*","arn:aws:s3:::qd-tenant-*/*"]},{"Effect":"Allow","Action":["admin:*"],"Resource":["arn:aws:s3:::*"]}]}'
AK=qd-platform-admin
expect "the user does not exist yet (user-info)" 404 "$(call root GET "/rustfs/admin/v3/user-info?accessKey=$AK")"
expect "the policy qd-platform-admin (add-canned-policy)" 200 "$(call root PUT /rustfs/admin/v3/add-canned-policy?name=qd-platform-admin "$POLICY")"
# The secret's lengths RustFS takes, on throwaway users.
lengths=''
for n in 8 40 41 64 128; do
  s=$(openssl rand -hex 64 | cut -c1-$n)
  lengths="$lengths $n:$(call root PUT "/rustfs/admin/v3/add-user?accessKey=qd-probe-$n" "{\"secretKey\":\"$s\",\"status\":\"enabled\"}")"
done
expect "the secret's lengths add-user takes" " 8:200 40:200 41:200 64:200 128:200" "$lengths"
# The secret: 64 hex digits from the system's CSPRNG, as join.sh makes it.
printf '%s %s\n' "$AK" "$(od -An -tx1 -N32 /dev/urandom | tr -d ' \n')" >"$D/admin"
read -r _ SECRET <"$D/admin"
expect "it is 64 hex digits" 1 "$(printf '%s' "$SECRET" | grep -cE '^[0-9a-f]{64}$')"
expect "the user (add-user)" 200 "$(call root PUT "/rustfs/admin/v3/add-user?accessKey=$AK" "{\"secretKey\":\"$SECRET\",\"status\":\"enabled\"}")"
expect "the policy attached (set-user-or-group-policy)" 200 \
  "$(call root PUT "/rustfs/admin/v3/set-user-or-group-policy?policyName=qd-platform-admin&userOrGroup=$AK&isGroup=false")"
expect "the user exists now (user-info)" 200 "$(call root GET "/rustfs/admin/v3/user-info?accessKey=$AK")"

echo "== (c) what the user does, with its own keys alone"
expect "the admin API: info" 200 "$(call admin GET /rustfs/admin/v3/info)"
expect "the admin API: list-users" 200 "$(call admin GET /rustfs/admin/v3/list-users)"
expect "a tenant's bucket" 200 "$(call admin PUT /qd-tenant-verify)"
expect "its table bucket (Iceberg)" 200 "$(call admin PUT /iceberg/v1/buckets/qd-tenant-verify)"
expect "versioning on it" 200 "$(call admin PUT '/qd-tenant-verify?versioning' '<VersioningConfiguration xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Status>Enabled</Status></VersioningConfiguration>')"
expect "a bucket outside qd-tenant-* is denied" 403 "$(call admin PUT /other-verify)"
expect "a pool's policy" 200 "$(call admin PUT /rustfs/admin/v3/add-canned-policy?name=qd-pool-verify '{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:GetObject"],"Resource":["arn:aws:s3:::qd-tenant-verify/*"]}]}')"
printf 'qd-pool-verify %s\n' "$(od -An -tx1 -N32 /dev/urandom | tr -d ' \n')" >"$D/pool"
read -r _ pool_secret <"$D/pool"
expect "a pool's user" 200 "$(call admin PUT /rustfs/admin/v3/add-user?accessKey=qd-pool-verify "{\"secretKey\":\"$pool_secret\",\"status\":\"enabled\"}")"
expect "its policy attached" 200 "$(call admin PUT '/rustfs/admin/v3/set-user-or-group-policy?policyName=qd-pool-verify&userOrGroup=qd-pool-verify&isGroup=false')"
expect "the pool's user, GetObject alone, is refused the bucket's listing" 403 "$(call pool GET /qd-tenant-verify)"

echo "== (d) a second run"
expect "user-info tells it exists, before anything is made" 200 "$(call root GET "/rustfs/admin/v3/user-info?accessKey=$AK")"
expect "the policy made again: the same answer" 200 "$(call root PUT /rustfs/admin/v3/add-canned-policy?name=qd-platform-admin "$POLICY")"
printf '%s %s\n' "$AK" "$(od -An -tx1 -N32 /dev/urandom | tr -d ' \n')" >"$D/admin2"
read -r _ second <"$D/admin2"
expect "add-user of an existing user is not refused" 200 "$(call root PUT "/rustfs/admin/v3/add-user?accessKey=$AK" "{\"secretKey\":\"$second\",\"status\":\"enabled\"}")"
expect "and replaces its secret: the first one refused, the second takes" "403 200" \
  "$(call admin GET /rustfs/admin/v3/info) $(call admin2 GET /rustfs/admin/v3/info)"
expect "the policy still attached after it" 200 "$(call admin2 PUT /qd-tenant-verify2)"

echo "== (e) what Docker keeps"
logs=$(docker logs "$C" 2>&1)
found=0
for f in admin admin2 pool; do
  read -r _ s <"$D/$f"
  case $logs in *"$s"*) found=$((found + 1)) ;; esac
done
. "$D/root-keys.env"
case $logs in *"$RUSTFS_SECRET_KEY"*) found=$((found + 1)) ;; esac
expect "no secret, the root's either, in RustFS's log (level info)" 0 "$found"
expect "no exec's answer in it either" 0 "$(printf '%s' "$logs" | grep -c 'qd-platform-admin')"

[ $FAILED = 0 ] && echo "ALL OK" || echo "SOME FAILED"
exit $FAILED
