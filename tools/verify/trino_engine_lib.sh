# shellcheck shell=bash
# Shared helpers of decision 0090's verification scripts (trino_*.sh with
# Trino 484 as the engine): a throwaway network, RustFS 1.0.5 with one
# warehouse made by the platform admin user (never root), and one small
# Trino 484 node (2 GB heap) with the catalogs `lake` (read and write),
# `gold` (the same warehouse, read-only), `memory` and `tpch`, plus any
# catalog a script writes into "$D/catalog". No port is published: every
# query runs through `docker exec` and the Trino CLI. Synthetic rows only;
# keys are random, kept in files under "$D" and never printed. Not run by
# itself: a script sets P (qd-verify-…) and sources this file.
#
# A script may write "$D/rules.json" (file-based access control) before
# te_trino; without it every user may do everything.

: "${P:?set P to qd-verify-<name> before sourcing}"
TRINO_IMAGE=${TRINO_IMAGE:-trinodb/trino:484}
RUSTFS_IMAGE=${RUSTFS_IMAGE:-rustfs/rustfs:1.0.5}
TRINO_HEAP=${TRINO_HEAP:-2G}
WAREHOUSE=${WAREHOUSE:-qd-tenant-00000000000000000000000000000090}
FAILED=0
D=$(mktemp -d)
chmod 700 "$D"
mkdir -p "$D/catalog"

ok() { printf '[OK  ] %s\n' "$*"; }
ko() { printf '[FAIL] %s\n' "$*"; FAILED=1; }
info() { printf '[INFO] %s\n' "$*"; }
expect() { # NAME EXPECTED ACTUAL
  if [ "$2" = "$3" ]; then ok "$1: $3"; else ko "$1: expected '$2', got '$3'"; fi
}
# expect_match NAME REGEX ACTUAL
expect_match() {
  if printf '%s' "$3" | grep -Eq "$2"; then ok "$1: $(printf '%s' "$3" | head -c 200)"; else
    ko "$1: expected /$2/, got '$(printf '%s' "$3" | tr '\n' ' ' | head -c 300)'"
  fi
}

te_cleanup() {
  docker ps -aq --filter "name=^$P-" | xargs -r docker rm -f -v >/dev/null 2>&1
  docker volume ls -q --filter "name=^$P-" | xargs -r docker volume rm >/dev/null 2>&1
  docker network rm "$P" >/dev/null 2>&1
  rm -rf "$D"
}
trap te_cleanup EXIT
trap 'exit 130' INT TERM
docker network create "$P" >/dev/null || exit 1

# --- RustFS: the lake, one warehouse, the platform admin user -----------------
te_rustfs() {
  local policy secret code call
  printf 'RUSTFS_ACCESS_KEY=qd%s\nRUSTFS_SECRET_KEY=%s\n' "$(openssl rand -hex 8)" "$(openssl rand -hex 20)" >"$D/root.env"
  docker run -d --name "$P-lake" --network "$P" --memory 1g --env-file "$D/root.env" \
    -e RUSTFS_VOLUMES=/data "$RUSTFS_IMAGE" server --address :9000 >/dev/null || return 1
  for _ in $(seq 1 30); do
    docker exec "$P-lake" curl -s -o /dev/null http://127.0.0.1:9000/health 2>/dev/null && break
    sleep 1
  done
  policy=$(sed -n "s/^ADMIN_POLICY='\(.*\)'$/\1/p" infra/rustfs/admin-user.sh)
  [ -n "$policy" ] || { echo "no ADMIN_POLICY in infra/rustfs/admin-user.sh"; return 1; }
  secret=$(openssl rand -hex 24)
  printf 'LAKE_ACCESS_KEY=qd-platform-admin\nLAKE_SECRET_KEY=%s\n' "$secret" >"$D/admin.env"
  for call in "PUT /rustfs/admin/v3/add-canned-policy?name=qd-platform-admin $policy" \
    "PUT /rustfs/admin/v3/add-user?accessKey=qd-platform-admin {\"secretKey\":\"$secret\",\"status\":\"enabled\"}" \
    "PUT /rustfs/admin/v3/set-user-or-group-policy?policyName=qd-platform-admin&userOrGroup=qd-platform-admin&isGroup=false "; do
    # shellcheck disable=SC2086
    set -- $call
    code=$(te_lake_call "$1" "$2" "${3:-}" root)
    [ "$code" = 200 ] || { echo "admin user: $2 answered $code"; return 1; }
  done
  unset secret
  for call in "PUT /$WAREHOUSE" "PUT /iceberg/v1/buckets/$WAREHOUSE"; do
    # shellcheck disable=SC2086
    set -- $call
    code=$(te_lake_call "$1" "$2" "" admin)
    [ "$code" = 200 ] || { echo "warehouse: $2 answered $code"; return 1; }
  done
}
# te_lake_call METHOD PATH BODY root|admin: a SigV4 call from inside the
# lake's container; prints the HTTP code (the body goes to "$D/lake.out").
te_lake_call() {
  local env=$D/root.env ak=RUSTFS_ACCESS_KEY sk=RUSTFS_SECRET_KEY
  [ "$4" = admin ] && { env=$D/admin.env; ak=LAKE_ACCESS_KEY; sk=LAKE_SECRET_KEY; }
  printf 'url = "http://127.0.0.1:9000%s"\nrequest = "%s"\ndata-binary = "%s"\n' "$2" "$1" \
    "$(printf '%s' "$3" | sed 's/"/\\"/g')" |
    docker exec -i --env-file "$env" "$P-lake" sh -c "curl -sS -o /tmp/out -w '%{http_code}' \
      --variable %$ak --variable %$sk --expand-user '{{$ak}}:{{$sk}}' \
      --aws-sigv4 aws:amz:us-east-1:s3 -K -; cat /tmp/out >&2" 2>"$D/lake.out"
}

# --- Trino 484, one small node -------------------------------------------------
te_lake_catalog() { # NAME [extra properties]
  cat >"$D/catalog/$1.properties" <<PROPS
connector.name=iceberg
iceberg.catalog.type=rest
iceberg.rest-catalog.uri=http://$P-lake:9000/iceberg
iceberg.rest-catalog.warehouse=$WAREHOUSE
iceberg.rest-catalog.security=SIGV4
iceberg.rest-catalog.signing-name=s3
iceberg.rest-catalog.view-endpoints-enabled=true
fs.native-s3.enabled=true
s3.endpoint=http://$P-lake:9000
s3.region=us-east-1
s3.path-style-access=true
s3.aws-access-key=\${ENV:LAKE_ACCESS_KEY}
s3.aws-secret-key=\${ENV:LAKE_SECRET_KEY}
${2:-}
PROPS
}
te_trino() {
  [ -f "$D/admin.env" ] || : >"$D/admin.env"
  if [ -f "$D/root.env" ]; then
    te_lake_catalog lake
    te_lake_catalog gold "iceberg.security=read_only"
  fi
  printf 'connector.name=memory\nmemory.max-data-per-node=256MB\n' >"$D/catalog/memory.properties"
  printf 'connector.name=tpch\n' >"$D/catalog/tpch.properties"
  cat >"$D/config.properties" <<'PROPS'
coordinator=true
node-scheduler.include-coordinator=true
http-server.http.port=8080
discovery.uri=http://localhost:8080
query.max-memory=1GB
query.max-memory-per-node=1GB
PROPS
  [ -n "${TRINO_CONFIG:-}" ] && printf '%s\n' "$TRINO_CONFIG" >>"$D/config.properties"
  printf -- '-server\n-Xmx%s\n-XX:+UseG1GC\n-XX:+ExitOnOutOfMemoryError\n-Djdk.attach.allowAttachSelf=true\n' \
    "$TRINO_HEAP" >"$D/jvm.config"
  local acl=()
  if [ -f "$D/rules.json" ]; then
    printf 'access-control.name=file\nsecurity.config-file=/etc/trino/rules.json\nsecurity.refresh-period=1s\n' \
      >"$D/access-control.properties"
    chmod 644 "$D/rules.json"
    acl=(-v "$D/access-control.properties:/etc/trino/access-control.properties:ro"
      -v "$D/rules.json:/etc/trino/rules.json:ro")
  fi
  chmod 755 "$D" "$D/catalog"
  chmod 644 "$D"/catalog/* "$D/config.properties" "$D/jvm.config"
  # TRINO_PUBLISH=1: the HTTP port on a random port of 127.0.0.1 (te_trino_url).
  [ "${TRINO_PUBLISH:-0}" = 1 ] && acl+=(-p 127.0.0.1::8080)
  docker run -d --name "$P-trino" --network "$P" --memory 3g --env-file "$D/admin.env" \
    -v "$D/config.properties:/etc/trino/config.properties:ro" \
    -v "$D/jvm.config:/etc/trino/jvm.config:ro" \
    -v "$D/catalog:/etc/trino/catalog:ro" "${acl[@]}" \
    "$TRINO_IMAGE" >/dev/null || return 1
  chmod 700 "$D"
  for _ in $(seq 1 90); do
    docker logs "$P-trino" 2>&1 | grep -q "SERVER STARTED" && return 0
    docker ps -q --filter "name=^$P-trino$" | grep -q . || break
    sleep 2
  done
  echo "Trino did not start:"; docker logs "$P-trino" 2>&1 | tail -20
  return 1
}
te_trino_url() { printf 'http://%s' "$(docker port "$P-trino" 8080/tcp | head -1)"; }
# tq USER SQL: runs SQL as USER; prints the TSV rows, or the error line.
# The exit status is 1 when the statement failed.
tq() {
  local out rc
  out=$(docker exec "$P-trino" trino --server localhost:8080 --user "$1" \
    --output-format TSV --execute "$2" 2>&1 | grep -v '^Picked up\|^WARNING')
  rc=0
  case "$out" in *"Query "*" failed:"* | *"Error running command"*) rc=1 ;; esac
  printf '%s\n' "$out"
  return $rc
}
# tq_ok NAME USER SQL: the statement must succeed.
tq_ok() {
  local out
  if out=$(tq "$2" "$3"); then ok "$1: $(printf '%s' "$out" | tr '\n' ' ' | head -c 200)"; else
    ko "$1: $(printf '%s' "$out" | tr '\n' ' ' | head -c 300)"
  fi
}
# tq_refused NAME USER SQL [REGEX]: the statement must fail (with REGEX).
tq_refused() {
  local out
  if out=$(tq "$2" "$3"); then ko "$1: expected a refusal, it ran: $(printf '%s' "$out" | head -c 200)"; else
    expect_match "$1 (refused)" "${4:-.}" "$out"
  fi
}
# tq_http USER SQL: the statement through the HTTP API with curl inside the
# container (no CLI JVM, for concurrent statements), polled to its end;
# prints FINISHED or "failed: <error name and message>", exit status 1 when
# it failed.
tq_http() {
  local r n e
  r=$(docker exec "$P-trino" curl -s -H "X-Trino-User: $1" --data-binary "$2" http://localhost:8080/v1/statement)
  while :; do
    e=$(printf '%s' "$r" | grep -o '"errorName":"[^"]*"\|"message":"[^"]\{0,160\}' | head -2 | tr '\n' ' ')
    case "$e" in *errorName*) ;; *) e= ;; esac
    [ -n "$e" ] && { echo "failed: $e"; return 1; }
    n=$(printf '%s' "$r" | grep -o '"nextUri":"[^"]*"' | sed 's/"nextUri":"//; s/"$//')
    [ -z "$n" ] && { echo FINISHED; return 0; }
    sleep 0.3
    r=$(docker exec "$P-trino" curl -s -H "X-Trino-User: $1" "$n")
  done
}
# te_log REGEX: the first lines of Trino's log matching REGEX.
te_log() { docker logs "$P-trino" 2>&1 | grep -E -m2 "$1" | cut -c1-300; }
