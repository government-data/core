#!/bin/sh
# Joins this server to a Quality Data installation (decisions 0029 and
# 0066). Settings → Execution shows the command that downloads it, checks
# its SHA-256 and runs it; run it as that command says, as root:
#
#   sh qd-join.sh --url 'https://<domain>' --role worker [--pool POOL] \
#     [--metrics-url 'https://<observability server>:9009/api/v1/push'] \
#     --ca-sha256 '<the CA's SHA-256>' --token 'qdj_…' [--name NAME] \
#     [--pin 'sha256//<key pin>']
#
# With --metrics-url (the command has it when the installation names where
# the machines' metrics go, decision 0071), the role's Alloy runs (the
# compose profile `metrics`) and pushes this machine's metrics there.
#
# The lake's replica (--role lake-replica) needs its drives too, each a path
# where a filesystem of its own is mounted, empty the first time:
# --drive /srv/disk1 [--drive /srv/disk2 …]. So does a node of a lake pool
# (--role lake --pool main --name NODE, as Settings → Lake gives it: the
# token names the node, and as many drives as the pool says).
#
# What it does, in this order, and nothing before its checks pass:
#   1. checks what it needs: root, curl, sha256sum, base64, Docker and its
#      compose plugin; for a lake role, its drives (each a directory where a
#      filesystem of its own is mounted, none the same, empty unless this
#      server's from a run before) and that the clock is in sync, which
#      stops it if not (for the worker, a warning);
#   2. downloads the installation's CA and checks its SHA-256 (of its DER)
#      against --ca-sha256: another CA stops it before the token is sent;
#      for a lake role, the installation's clock (its answer's Date) must
#      be within 30 seconds of this server's;
#   3. downloads its role's compose file and the files it names (its
#      entrypoints and its Alloy configuration, <role>.alloy), giving the
#      token in a header (the token is checked, not used), and checks each
#      against the SHA-256 this script names below; for a lake role, its
#      root keys too, made by the installation (never printed): a refused
#      token (401) stops it; for a pool's node, the pool's layout
#      (pool.env), which must name this server and as many drives as given,
#      and every other node's name (and the replica's, when known) must
#      resolve here to an IPv4 address of another server (DNS), or it stops;
#   4. writes them into QD_JOIN_DIR (default /opt/qd/<role>): compose.yml,
#      the role's entrypoints (0755) and Alloy configuration (0644), ca.crt,
#      .env (the agent's settings, OBS_VM and METRICS_PUSH_URL for Alloy,
#      and for a worker's Alloy Docker's data, QD_WATCH_DOCKER, its watched
#      path, as `docker info` gives it, decision 0072;
#      lines it does not own are kept) and join/token (0600, the agent's
#      user's; the agent deletes it once joined); for a lake role also
#      root-keys.env (0600), drives.yml (each drive in RustFS's container),
#      machine/ (the agent's directory) and otlp-token, the token of RustFS's
#      metrics push to the node's Alloy (0600, RustFS's user's; made here
#      from the system's CSPRNG, never printed, kept on a later run; decision
#      0071, the user's ruling of 2026-11-14); for a pool's node peers.yml,
#      the addresses DNS gave for those names, which Docker's resolver on a
#      compose network would not find (an address that changes needs the
#      command run again); with --metrics-url, metrics.yml, the same for
#      the observability server's name (resolved in step 3) and, for a
#      lake role, each drive in Alloy, read-only (decision 0072); each
#      written aside and renamed; a lake role's drives are given to RustFS's user (10001);
#   5. for a pool's node, writes Docker's firewall rules (DOCKER-USER) so
#      that its peers' port is open to the other nodes' addresses alone
#      (Docker's iptables backend needed: without it, it stops at step 1);
#      starts the role's compose: the node agent joins with the token and
#      the machine waits, pending, for a person to approve it;
#   6. for a lake role, once RustFS answers: makes the platform's admin user
#      (qd-platform-admin, decision 0028, point 2) with the root keys, its
#      secret 64 hex digits from the system's CSPRNG, and shows its keys
#      once, on this terminal alone, to be pasted in Settings → Lake; they
#      are written to no file. A user already there is not made again and
#      no secret is shown (its policy is set again). A pool answers once all
#      its nodes are up, and its user, the lake's, is made by one node
#      alone, the last of the pool's layout, so no two runs make it at once:
#      another node says so once RustFS answers; that one, when the pool has
#      not formed within the wait, says to run its command again once every
#      node is up.
#
# The token is sent only to a server authenticated by --pin, or by the
# system's roots without one, and only after the CA is checked. It is never
# printed, nor passed to another program's arguments. Run again, it writes
# the same files and starts again; with a token already used it stops at
# step 3 and changes nothing. QD_JOIN_NO_START=1 stops after step 4 and
# skips Docker's checks (for verification, where the image may not exist).
#
# Not built yet: the roles trino-coordinator and trino-worker,
# --upgrade and --remove, images by digest and the installation's registry,
# and the checks of CPU, memory, disk and ports.
set -eu

# The SHA-256 of each file of this version it downloads (a test keeps them
# in step with infra/worker/docker-compose.yml,
# infra/rustfs/replica/docker-compose.yml,
# infra/rustfs/pool/docker-compose.yml and the entrypoints
# infra/rustfs/tls/entrypoint.sh, infra/rustfs/gate/entrypoint.sh and
# infra/worker/lake/entrypoint.sh, and the Alloy configurations
# infra/observability/alloy/<role>.alloy).
LAKE_ALLOY_SHA256=02f615be38084100fbf9294a6d6175a757dbb5bacc1bfe3e551571577166bcd8
LAKE_REPLICA_ALLOY_SHA256=7e7e60858d985ed22e4a6f41b3610f2607abeeb82c2a4532bd6ae9da12acf27e
WORKER_ALLOY_SHA256=8fbbcad28b7863102e10e03e5309ded12d654260167c1dbc2a984e41f739132a
WORKER_COMPOSE_SHA256=a8213ef2d098a46be6b4413ae4629488b58e668a1678a082dc2ec531f973eec4
LAKE_REPLICA_COMPOSE_SHA256=f5f9765ac42e7333ef25d26868ae825aef94fe9242b10f634c7103cadaed56af
RUSTFS_TLS_SHA256=300e8d0fd49caf0ccc6144a6dce99b40219dd4ad73d30f88dd3788f8d4b94147
LAKE_GATE_SHA256=b824342357c5725edba706d46a3fbdfd14e38e2fe7183758e312c479e3e3872f
LAKE_POOL_COMPOSE_SHA256=3cf224e832388df396e11acbabdc78f162ad5db986fe6c4454d764afe58a2a0b
LAKE_PROXY_SHA256=0dc6fdba9f22d70f0f85af6d4145ecc5afec594afc9f2e5645f57545cdc4d2bb
# RustFS's user in its image, which must own a lake's drives.
RUSTFS_UID=10001
# Most seconds this server's clock may be off the installation's (a lake).
MAX_SKEW=30
# A lake replica's platform admin user (decision 0028, point 2) and its
# policy: every action on the warehouse buckets and the admin API.
ADMIN_USER=qd-platform-admin
ADMIN_POLICY='{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:*"],"Resource":["arn:aws:s3:::qd-tenant-*","arn:aws:s3:::qd-tenant-*/*"]},{"Effect":"Allow","Action":["admin:*"],"Resource":["arn:aws:s3:::*"]}]}'
# Most seconds to wait for RustFS to answer once started (its entrypoint
# waits up to 300 for the agent's certificate).
LAKE_WAIT=${QD_JOIN_LAKE_WAIT:-360}

say() { printf 'qd-join: %s\n' "$*"; }
die() {
  printf 'qd-join: %s\n' "$*" >&2
  exit 1
}
usage() {
  printf '%s\n' "usage: sh qd-join.sh --url URL --role ROLE [--pool POOL] [--metrics-url URL] --ca-sha256 HEX --token TOKEN [--name NAME] [--pin sha256//PIN] [--drive PATH …]" >&2
  exit 2
}

URL='' ROLE='' POOL='' NAME='' CA_SHA256='' TOKEN='' PIN='' DRIVES='' POOLED='' METRICS_URL='' POOLED_OR_METRICS=''
while [ $# -gt 0 ]; do
  case $1 in
  --url | --role | --pool | --name | --ca-sha256 | --token | --pin | --drive | --metrics-url)
    [ $# -ge 2 ] || usage
    case $1 in
    --url) URL=$2 ;;
    --role) ROLE=$2 ;;
    --pool) POOL=$2 ;;
    --name) NAME=$2 ;;
    --ca-sha256) CA_SHA256=$2 ;;
    --token) TOKEN=$2 ;;
    --pin) PIN=$2 ;;
    --drive) DRIVES="$DRIVES ${2%/}" ;;
    --metrics-url) METRICS_URL=$2 ;;
    esac
    shift 2
    ;;
  --upgrade | --remove) die "$1 is not built yet: nothing was done" ;;
  *) usage ;;
  esac
done

[ -n "$URL" ] && [ -n "$ROLE" ] && [ -n "$CA_SHA256" ] && [ -n "$TOKEN" ] || usage
# Per role: its compose file's hash, the files it names (name:sha256), the
# node agent's user, and whether it is a lake node.
case $ROLE in
worker) COMPOSE_SHA256=$WORKER_COMPOSE_SHA256 FILES="worker.alloy:$WORKER_ALLOY_SHA256" AGENT_UID=65532 LAKE='' ;;
lake-replica)
  COMPOSE_SHA256=$LAKE_REPLICA_COMPOSE_SHA256 AGENT_UID=$RUSTFS_UID LAKE=1
  FILES="rustfs-tls.sh:$RUSTFS_TLS_SHA256 lake-gate.sh:$LAKE_GATE_SHA256 lake-replica.alloy:$LAKE_REPLICA_ALLOY_SHA256"
  ;;
lake)
  COMPOSE_SHA256=$LAKE_POOL_COMPOSE_SHA256 AGENT_UID=$RUSTFS_UID LAKE=1 POOLED=1
  FILES="rustfs-tls.sh:$RUSTFS_TLS_SHA256 lake-gate.sh:$LAKE_GATE_SHA256 lake-proxy.sh:$LAKE_PROXY_SHA256 lake.alloy:$LAKE_ALLOY_SHA256"
  [ -n "$POOL" ] && [ -n "$NAME" ] || die "a pool's node needs --pool and --name, as Settings → Lake gives them; nothing was done"
  POOLED_OR_METRICS=1
  ;;
trino-coordinator | trino-worker)
  die "the role $ROLE cannot join with this script yet: only worker, lake-replica and lake; nothing was done" ;;
*) die "unknown role: $ROLE" ;;
esac
case $URL in
https://*) ;;
*) die "--url must be https://…: the token never travels unencrypted" ;;
esac
URL=${URL%/}
case $METRICS_URL in
'') ;;
https://*)
  printf '%s' "$METRICS_URL" | grep -Eq '^https://[A-Za-z0-9._~:/-]+$' ||
    die "--metrics-url has characters it cannot have: $METRICS_URL"
  ;;
*) die "--metrics-url must be https://…: the metrics never travel unencrypted" ;;
esac
HOST=${URL#https://}
HOST=${HOST%%/*}
HOST=${HOST%%:*}
label='^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$'
if [ -z "$NAME" ]; then
  NAME=$(uname -n | cut -d. -f1 | tr '[:upper:]' '[:lower:]')
fi
printf '%s' "$NAME" | grep -Eq "$label" ||
  die "the machine's name '$NAME' is not lowercase letters, digits and hyphens (1 to 63): give one with --name"
[ -z "$POOL" ] || printf '%s' "$POOL" | grep -Eq "$label" || die "a bad pool: $POOL"
CA_SHA256=$(printf '%s' "$CA_SHA256" | tr -d ':' | tr '[:upper:]' '[:lower:]')
printf '%s' "$CA_SHA256" | grep -Eq '^[0-9a-f]{64}$' || die "--ca-sha256 is not a SHA-256"
DIR=${QD_JOIN_DIR:-/opt/qd/$ROLE}

# 1. Prerequisites.
[ "$(id -u)" = 0 ] || die "run it as root: it writes $DIR and the agent's token for its user"
[ -z "$METRICS_URL" ] || POOLED_OR_METRICS=1
for tool in curl sha256sum base64 sed stat date od ${POOLED_OR_METRICS:+getent}; do
  command -v "$tool" >/dev/null 2>&1 || die "$tool is missing"
done
if [ "${QD_JOIN_NO_START:-}" != 1 ]; then
  command -v docker >/dev/null 2>&1 || die "Docker is missing"
  docker compose version >/dev/null 2>&1 || die "Docker's compose plugin is missing"
  docker info >/dev/null 2>&1 || die "Docker is not running"
  # A worker's watched path, Docker's data, as Docker says (decision 0072).
  [ "$ROLE" != worker ] || WATCH_DOCKER=$(docker info -f '{{.DockerRootDir}}')
  # A pool's node keeps its peers' port to the pool's nodes with Docker's
  # own firewall chain, which only Docker's iptables backend has.
  if [ -n "$POOLED" ]; then
    for tool in iptables iptables-restore; do
      command -v "$tool" >/dev/null 2>&1 || die "$tool is missing: a pool's node keeps its peers' port to the pool's nodes with it; nothing was done"
    done
    iptables -S DOCKER-USER >/dev/null 2>&1 && iptables -C FORWARD -j DOCKER-USER >/dev/null 2>&1 ||
      die "Docker's firewall chain DOCKER-USER is not in use here (Docker's nftables backend, or its iptables off): a pool's node keeps its peers' port to the pool's nodes with it; nothing was done"
  fi
fi
if command -v timedatectl >/dev/null 2>&1 &&
  [ "$(timedatectl show -p NTPSynchronized --value 2>/dev/null)" != yes ]; then
  [ -z "$LAKE" ] || die "the clock is not in sync (timedatectl): a lake node needs it; nothing was done"
  say "warning: the clock is not in sync; certificates need it"
fi
# empty DIR: nothing in it but a filesystem's lost+found.
empty() {
  for e in "$1"/* "$1"/.[!.]* "$1"/..?*; do
    [ -e "$e" ] || [ -L "$e" ] || continue
    [ "$e" = "$1/lost+found" ] || return 1
  done
}
# A lake's drives: this server's from a run before may hold RustFS's data.
if [ -n "$LAKE" ]; then
  [ -n "$DRIVES" ] || die "give each drive with --drive PATH (a filesystem of its own, mounted there, empty); nothing was done"
  before=$(sed -n 's/^QD_LAKE_DRIVES=//p' "$DIR/.env" 2>/dev/null || true)
  seen=''
  for d in $DRIVES; do
    printf '%s' "$d" | grep -Eq '^/[A-Za-z0-9._/-]+$' && ! printf '%s' "$d" | grep -Eq '(^|/)\.\.?(/|$)' ||
      die "a drive must be an absolute path of letters, digits, . _ - and /: $d"
    [ -d "$d" ] || die "the drive $d is not a directory; nothing was done"
    dev=$(stat -c %d "$d")
    [ "$dev" != "$(stat -c %d "$d/..")" ] && [ "$dev" != "$(stat -c %d /)" ] ||
      die "the drive $d is not a filesystem of its own mounted there; nothing was done"
    case " $seen " in *" $dev "*) die "the drive $d is on the same filesystem as another; nothing was done" ;; esac
    seen="$seen $dev"
    case " $before " in
    *" $d "*) ;;
    *) empty "$d" || die "the drive $d is not empty; nothing was done" ;;
    esac
  done
  DRIVES=${DRIVES# }
fi

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT INT TERM
chmod 700 "$TMP"
# The TLS of every request: the pinned key with -k, or the system's roots.
fetch() {
  if [ -n "$PIN" ]; then
    curl -sS --proto '=https' --tlsv1.2 --max-time 60 -k --pinnedpubkey "$PIN" "$@"
  else
    curl -sS --proto '=https' --tlsv1.2 --max-time 60 "$@"
  fi
}
# epoch DATE: the seconds of an HTTP date, with GNU's date or BusyBox's.
epoch() {
  [ -n "$1" ] || return 1
  date -u -d "$1" +%s 2>/dev/null ||
    date -u -D '%a, %d %b %Y %H:%M:%S' -d "${1% GMT}" +%s 2>/dev/null
}

# 2. The CA, checked before the token goes anywhere.
code=$(fetch -D "$TMP/ca.head" -o "$TMP/ca.crt" -w '%{http_code}' "$URL/api/install/ca.crt") ||
  die "the installation could not be reached at $URL"
[ "$code" = 200 ] || die "the installation gave no CA ($code)"
got=$(sed -n '/-----BEGIN CERTIFICATE-----/,/-----END CERTIFICATE-----/p' "$TMP/ca.crt" |
  sed '/-----/d' | tr -d '\r\n' | base64 -d 2>/dev/null | sha256sum | cut -d' ' -f1)
[ "$got" = "$CA_SHA256" ] || die "the CA is not the one of --ca-sha256: stopped, the token was not sent"
say "the installation's CA checked"
if [ -n "$LAKE" ]; then
  theirs=$(epoch "$(sed -n 's/^[Dd]ate: *//p' "$TMP/ca.head" | tr -d '\r' | head -1)") ||
    die "the installation's time could not be read: stopped, the token was not sent"
  skew=$(($(date -u +%s) - theirs))
  [ "${skew#-}" -le "$MAX_SKEW" ] ||
    die "the clock is ${skew}s off the installation's (more than ${MAX_SKEW}s): put it in sync; the token was not sent"
fi

# v4 NAME: NAME's IPv4 addresses by this server's DNS, one a line.
v4() { getent ahostsv4 "$1" 2>/dev/null | sed 's/[[:space:]].*//' | grep -E '^[0-9]+(\.[0-9]+){3}$' | grep -v '^127\.' | sort -u || true; }

# 3. The role's files, for the token, checked.
printf 'Authorization: Bearer %s\n' "$TOKEN" >"$TMP/auth"
# get PATH FILE: the installation's PATH, for the token, into TMP/FILE.
get() {
  code=$(fetch -o "$TMP/$2" -w '%{http_code}' -H @"$TMP/auth" "$URL/api/install/$ROLE/$1") ||
    die "the installation could not be reached at $URL"
  case $code in
  200) ;;
  401) die "the token is unknown, used or expired: ask for a new command in Settings → Execution; nothing was changed" ;;
  429) die "too many attempts from this address: run it again in a minute; nothing was changed" ;;
  *) die "the installation gave no $1 for $ROLE ($code); nothing was changed" ;;
  esac
}
# checked FILE SHA256 WHAT
checked() {
  [ "$(sha256sum "$TMP/$1" | cut -d' ' -f1)" = "$2" ] ||
    die "$3 is not this version's: stopped, nothing was changed"
}
get compose.yml compose.yml
checked compose.yml "$COMPOSE_SHA256" "the compose file"
for f in $FILES; do
  get "files/${f%%:*}" "${f%%:*}"
  checked "${f%%:*}" "${f#*:}" "${f%%:*}"
done
say "the $ROLE compose file${FILES:+ and its files} checked"
if [ -n "$LAKE" ]; then
  get root-keys.env root-keys.env
  [ "$(grep -cE '^RUSTFS_ACCESS_KEY=[A-Za-z0-9]{3,64}$|^RUSTFS_SECRET_KEY=[A-Za-z0-9]{8,128}$' "$TMP/root-keys.env")" = 2 ] &&
    [ "$(wc -l <"$TMP/root-keys.env")" -eq 2 ] ||
    die "the installation's root keys are not of their form: stopped, nothing was changed"
fi
if [ -n "$POOLED" ]; then
  get pool.env pool.env
  ! grep -Evq '^(QD_POOL=[a-z0-9-]+|QD_POOL_NODE=[a-z0-9-]+|QD_POOL_NODES="[a-z0-9 -]+"|QD_POOL_DRIVES=[0-9]+|RUSTFS_VOLUMES="[a-z0-9:/. -]+"|QD_LAKE_REPLICA_GATE=[a-z0-9-]+:[0-9]+)$' "$TMP/pool.env" ||
    die "the pool's layout is not of its form: stopped, nothing was changed"
  layout() { sed -n "s/^$1=//p" "$TMP/pool.env" | tr -d '"'; }
  [ "$(layout QD_POOL)" = "$POOL" ] && [ "$(layout QD_POOL_NODE)" = "$NAME" ] ||
    die "the token is for node $(layout QD_POOL_NODE) of pool $(layout QD_POOL), not $NAME of $POOL: run its command on that server; nothing was changed"
  set -- $DRIVES
  [ "$#" = "$(layout QD_POOL_DRIVES)" ] ||
    die "the pool's nodes have $(layout QD_POOL_DRIVES) drives each and $# were given: nothing was changed"
  # Docker's resolver on a compose network does not pass a one-label name
  # on to this server's DNS, so each name is resolved here, by that DNS, and
  # given to the containers (peers.yml); its addresses are also the only ones
  # the peers' port is opened to (step 4).
  PEER_HOSTS='' PEER_ADDRS=''
  for peer in $(layout QD_POOL_NODES); do
    [ "$peer" != "$NAME" ] || continue
    addrs=$(v4 "$peer")
    [ -n "$addrs" ] ||
      die "the pool's node $peer does not resolve here to an address of another server (DNS, IPv4): the pool forms only when every node's name resolves on every other; nothing was changed"
    PEER_HOSTS="$PEER_HOSTS $peer:$(printf '%s\n' "$addrs" | head -1)"
    PEER_ADDRS="$PEER_ADDRS $(echo $addrs)"
  done
  REPLICA_GATE=$(layout QD_LAKE_REPLICA_GATE)
  if [ -n "$REPLICA_GATE" ]; then
    addrs=$(v4 "${REPLICA_GATE%:*}")
    [ -n "$addrs" ] ||
      die "the lake's replica ${REPLICA_GATE%:*} does not resolve here (DNS, IPv4): its replication needs it; nothing was changed"
    PEER_HOSTS="$PEER_HOSTS ${REPLICA_GATE%:*}:$(printf '%s\n' "$addrs" | head -1)"
  fi
  say "the pool's layout checked: $NAME of $POOL, every other node resolves"
fi
# Where the metrics go is resolved here too, for the same reason: Alloy
# reaches it by its name, which its certificate names (metrics.yml).
if [ -n "$METRICS_URL" ]; then
  MHOST=${METRICS_URL#https://}
  MHOST=${MHOST%%/*}
  MHOST=${MHOST%:*}
  METRICS_HOSTS=''
  if ! printf '%s' "$MHOST" | grep -Eq '^[0-9]+(\.[0-9]+){3}$'; then
    addrs=$(v4 "$MHOST")
    [ -n "$addrs" ] ||
      die "the observability server $MHOST does not resolve here (DNS, IPv4): this machine's metrics go there; nothing was changed"
    METRICS_HOSTS="$MHOST:$(printf '%s\n' "$addrs" | head -1)"
  fi
fi
rm -f "$TMP/auth"

# 4. The files, each written aside and renamed.
umask 077
mkdir -p "$DIR/join"
chmod 700 "$DIR"
# put SOURCE NAME MODE: SOURCE as DIR/NAME.
put() {
  cp "$1" "$DIR/.$2.new"
  chmod "$3" "$DIR/.$2.new"
  mv "$DIR/.$2.new" "$DIR/$2"
}
put "$TMP/compose.yml" compose.yml 644
for f in $FILES; do
  case ${f%%:*} in
  *.sh) put "$TMP/${f%%:*}" "${f%%:*}" 755 ;;
  *) put "$TMP/${f%%:*}" "${f%%:*}" 644 ;;
  esac
done
put "$TMP/ca.crt" ca.crt 644
COMPOSE_FILES="-f $DIR/compose.yml"
if [ -n "$LAKE" ]; then
  put "$TMP/root-keys.env" root-keys.env 600
  [ -z "$POOLED" ] || put "$TMP/pool.env" pool.env 644
  {
    printf '# Written by join.sh from --drive: each drive at /data/<n> in RustFS.\n'
    printf 'services:\n  rustfs:\n'
    # A pool's RUSTFS_VOLUMES is the pool's, in pool.env; a replica's its own.
    if [ -z "$POOLED" ]; then
      printf '    environment:\n      RUSTFS_VOLUMES: "'
      n=0 volumes=''
      for d in $DRIVES; do
        n=$((n + 1))
        volumes="$volumes /data/$n"
      done
      printf '%s"\n' "${volumes# }"
    fi
    printf '    volumes:\n'
    n=0
    for d in $DRIVES; do
      n=$((n + 1))
      printf '      - "%s:/data/%s"\n' "$d" "$n"
    done
  } >"$TMP/drives.yml"
  put "$TMP/drives.yml" drives.yml 644
  COMPOSE_FILES="$COMPOSE_FILES -f $DIR/drives.yml"
  if [ -n "$POOLED" ]; then
    {
      printf '# Written by join.sh: the pool'"'"'s other nodes (and the replica'"'"'s gate) at\n'
      printf '# the addresses this server'"'"'s DNS gave; if one changes, run the command again.\n'
      printf 'services:\n  rustfs:\n    extra_hosts:\n'
      for h in $PEER_HOSTS; do printf '      - "%s"\n' "$h"; done
    } >"$TMP/peers.yml"
    put "$TMP/peers.yml" peers.yml 644
    COMPOSE_FILES="$COMPOSE_FILES -f $DIR/peers.yml"
  fi
  for d in $DRIVES; do
    chown "$RUSTFS_UID:$RUSTFS_UID" "$d"
    chmod 750 "$d"
  done
  mkdir -p "$DIR/machine"
  chown "$AGENT_UID:$AGENT_UID" "$DIR/machine"
  chmod 700 "$DIR/machine"
  # RustFS's metrics push carries this token, which the node's Alloy
  # requires (decision 0071): made once, here, and kept.
  if ! grep -Eqx '[0-9a-f]{64}' "$DIR/otlp-token" 2>/dev/null; then
    od -An -tx1 -N32 /dev/urandom | tr -d ' \n' >"$DIR/.otlp-token.new"
    grep -Eqx '[0-9a-f]{64}' "$DIR/.otlp-token.new" || die "no token could be made from /dev/urandom"
    chmod 600 "$DIR/.otlp-token.new"
    chown "$RUSTFS_UID:$RUSTFS_UID" "$DIR/.otlp-token.new"
    mv "$DIR/.otlp-token.new" "$DIR/otlp-token"
  fi
  chown "$RUSTFS_UID:$RUSTFS_UID" "$DIR/otlp-token"
  chmod 600 "$DIR/otlp-token"
fi
if [ -n "$METRICS_URL" ]; then
  # A pool's node's Alloy is in RustFS's network namespace, so the name is
  # RustFS's to resolve.
  service=alloy
  [ -z "$POOLED" ] || service=rustfs
  {
    printf '# Written by join.sh: the observability server at the address this server'"'"'s\n'
    printf '# DNS gave, for Alloy (decision 0071); if it changes, run the command again.\n'
    printf 'services:\n  %s:\n    extra_hosts:' "$service"
    if [ -n "$METRICS_HOSTS" ]; then printf '\n      - "%s"\n' "$METRICS_HOSTS"; else printf ' []\n'; fi
    # A lake's Alloy watches each drive's filesystem, read-only, at
    # /host/watch/drive<n> (decision 0072, step 2).
    if [ -n "$LAKE" ]; then
      [ "$service" = alloy ] || printf '  alloy:\n'
      printf '    volumes:\n'
      n=0
      for d in $DRIVES; do
        n=$((n + 1))
        printf '      - "%s:/host/watch/drive%s:ro"\n' "$d" "$n"
      done
    fi
  } >"$TMP/metrics.yml"
  put "$TMP/metrics.yml" metrics.yml 644
  COMPOSE_FILES="$COMPOSE_FILES -f $DIR/metrics.yml"
fi
owned='^(QD_MACHINE_NAME|QD_CA_SHA256|QD_PUBLIC_URL|APP_DOMAIN|QD_JOIN_TOKEN_DIR|QD_LAKE_DRIVES|QD_LAKE_REPLICA_GATE|COMPOSE_PROFILES|OBS_VM|METRICS_PUSH_URL|QD_RUSTFS_METRICS|QD_WATCH_DOCKER)='
{
  [ -f "$DIR/.env" ] && grep -Ev "$owned" "$DIR/.env" || true
  printf 'QD_MACHINE_NAME=%s\nQD_CA_SHA256=%s\nQD_PUBLIC_URL=%s/api\nAPP_DOMAIN=%s\nQD_JOIN_TOKEN_DIR=%s/join\n' \
    "$NAME" "$CA_SHA256" "$URL" "$HOST" "$DIR"
  [ -z "$LAKE" ] || printf 'QD_LAKE_DRIVES=%s\n' "$DRIVES"
  # The machine's name on its series (and, later, its log lines).
  printf 'OBS_VM=%s\n' "$NAME"
  profiles=''
  # Alloy, when the command says where the metrics go.
  [ -z "$METRICS_URL" ] || { printf 'METRICS_PUSH_URL=%s\n' "$METRICS_URL"; profiles=metrics; }
  # Its Alloy's watched path, when Docker was asked (the compose file's
  # default, /var/lib/docker, otherwise).
  [ -z "$METRICS_URL" ] || [ -z "${WATCH_DOCKER:-}" ] || printf 'QD_WATCH_DOCKER=%s\n' "$WATCH_DOCKER"
  # A lake's RustFS pushes its own metrics to that Alloy.
  [ -z "$METRICS_URL" ] || [ -z "$LAKE" ] || printf 'QD_RUSTFS_METRICS=true\n'
  # A pool node's replication proxy, once the replica is known.
  [ -z "${REPLICA_GATE:-}" ] || { printf 'QD_LAKE_REPLICA_GATE=%s\n' "$REPLICA_GATE"; profiles="${profiles:+$profiles,}replication"; }
  [ -z "$profiles" ] || printf 'COMPOSE_PROFILES=%s\n' "$profiles"
} >"$TMP/env"
put "$TMP/env" .env 600
printf '%s' "$TOKEN" >"$DIR/join/.token.new"
chmod 600 "$DIR/join/.token.new"
chown "$AGENT_UID:$AGENT_UID" "$DIR/join" "$DIR/join/.token.new"
chmod 700 "$DIR/join"
mv "$DIR/join/.token.new" "$DIR/join/token"
say "written to $DIR as $NAME"

# 5. Started.
if [ "${QD_JOIN_NO_START:-}" = 1 ]; then
  say "not started (QD_JOIN_NO_START=1)"
  exit 0
fi
# A pool's peers' port, published on the server, open to the other nodes'
# addresses alone: a chain of this script's own, QD-LAKE-PEERS, replaced as
# a set at each run (one iptables-restore, no other rule touched), and one
# jump to it at the top of Docker's DOCKER-USER for new connections Docker's
# DNAT takes to 9010; the containers' own networks are not matched. Kept
# when Docker restarts, not across a reboot.
if [ -n "$POOLED" ]; then
  {
    printf '*filter\n:QD-LAKE-PEERS - [0:0]\n-F QD-LAKE-PEERS\n'
    for a in $PEER_ADDRS; do printf -- '-A QD-LAKE-PEERS -s %s/32 -j RETURN\n' "$a"; done
    printf -- '-A QD-LAKE-PEERS -j DROP\nCOMMIT\n'
  } | iptables-restore --noflush || die "the peers' port's rules could not be written: the $ROLE compose was not started"
  jump='DOCKER-USER -p tcp -m conntrack --ctstate DNAT --ctorigdstport 9010 --ctdir ORIGINAL -j QD-LAKE-PEERS'
  # shellcheck disable=SC2086 # the rule's words
  iptables -C $jump 2>/dev/null || iptables -I $jump ||
    die "the peers' port's rule could not be added to DOCKER-USER: the $ROLE compose was not started"
  say "the peers' port (9010) open to the pool's other nodes alone:$PEER_ADDRS"
fi
# shellcheck disable=SC2086 # the -f options: QD_JOIN_DIR has no spaces
docker compose --project-directory "$DIR" $COMPOSE_FILES --env-file "$DIR/.env" up -d --no-build ||
  die "the $ROLE compose did not start"
say "started: $NAME joins and waits, pending, for approval in Settings → Execution"
[ -n "$LAKE" ] || exit 0

# 6. The platform's admin user, made with the root keys by a curl run inside
# RustFS's container: it reads the root keys from that container's
# environment, everything else from its standard input (its config, -K -),
# so no key is in a program's arguments, and an exec's output is not in
# the container's log. RustFS is reached over TLS by the machine's name,
# its certificate checked against the CA; a pool's node on its peers' port,
# presenting its client certificate, which it requires.
# A run whose token joined cannot be repeated with it.
AGAIN="ask Settings → Execution for a new command and run it"
# s3 METHOD PATH [BODY]: the HTTP status of METHOD on RustFS's PATH.
RPORT=9000 CLIENT=''
[ -z "$POOLED" ] || RPORT=9010 CLIENT='cert = "/run/rustfs-tls/client_cert.pem"
key = "/run/rustfs-tls/client_key.pem"'
# shellcheck disable=SC2086 # the -f options, as above
s3() {
  {
    printf 'variable = "%%RUSTFS_ACCESS_KEY"\nvariable = "%%RUSTFS_SECRET_KEY"\n'
    printf 'expand-user = "{{RUSTFS_ACCESS_KEY}}:{{RUSTFS_SECRET_KEY}}"\naws-sigv4 = "aws:amz:us-east-1:s3"\n'
    printf 'resolve = "%s:%s:127.0.0.1"\ncacert = "/run/rustfs-tls/ca.crt"\n' "$NAME" "$RPORT"
    [ -z "$CLIENT" ] || printf '%s\n' "$CLIENT"
    printf 'silent\nmax-time = 20\noutput = "/dev/null"\nwrite-out = "%%{http_code}"\n'
    printf 'request = "%s"\nurl = "https://%s:%s%s"\n' "$1" "$NAME" "$RPORT" "$2"
    [ $# -lt 3 ] || printf 'data-binary = "%s"\n' "$(printf '%s' "$3" | sed 's/\\/\\\\/g; s/"/\\"/g')"
  } | docker compose --project-directory "$DIR" $COMPOSE_FILES --env-file "$DIR/.env" exec -T rustfs curl -K - 2>/dev/null
}
# The lake's admin user is made once, by one node: the last of the pool's
# layout (RustFS has no way to make a user only if it is not there, and two
# nodes finding the pool formed at once would each make it, the second
# replacing the first's secret). The others say so as soon as RustFS
# answers, formed (200) or waiting for its peers.
MAKER=$NAME
if [ -n "$POOLED" ]; then
  for n in $(layout QD_POOL_NODES); do MAKER=$n; done
  AGAIN="ask Settings → Lake for this node's command again and run it"
fi
UNFORMED="the pool has not formed yet: it does once all its nodes are up."
waited=0
while :; do
  code=$(s3 GET /) || true
  [ "$code" != 200 ] || break
  if [ "$MAKER" != "$NAME" ] && [ "$code" != 000 ]; then
    say "$UNFORMED The lake's admin user is made by the command of $MAKER, the last node of the pool, once it has formed"
    exit 0
  fi
  if [ "$waited" -ge "$LAKE_WAIT" ]; then
    [ -z "$POOLED" ] || [ "$code" = 000 ] ||
      { say "$UNFORMED This node, the last of the pool, makes the lake's admin user: once every node is up, $AGAIN"; exit 0; }
    die "RustFS did not answer within ${LAKE_WAIT}s, so the platform's admin user was not made: once it runs, $AGAIN"
  fi
  sleep 5
  waited=$((waited + 5))
done
if [ "$MAKER" != "$NAME" ]; then
  say "the pool has formed. The lake's admin user is made by the command of $MAKER alone, the last node of the pool: run it, again if it ran before the pool formed"
  exit 0
fi
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
    "qd-join: the platform's admin user of this lake, SHOWN ONLY THIS ONCE:" "$ADMIN_USER" "$secret" \
    "qd-join: paste both now in Quality Data, Settings → Lake. They are written to no file and" \
    "qd-join: cannot be shown again; this terminal is the only place they are."
  secret=''
  ;;
*) die "RustFS did not say whether the platform's admin user exists: $AGAIN" ;;
esac
[ "$(s3 PUT "/rustfs/admin/v3/set-user-or-group-policy?policyName=$ADMIN_USER&userOrGroup=$ADMIN_USER&isGroup=false")" = 200 ] ||
  die "RustFS did not give the platform's admin user its policy: $AGAIN"
say "the platform's admin user has its policy; the root keys are used for nothing else"
