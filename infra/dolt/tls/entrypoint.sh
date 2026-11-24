#!/bin/sh
# Dolt with the machine's certificate (decision 0066, point 11, step 10): the
# entrypoint of the service `dolt` in infra/dolt/certificate.yml, run as
# root (Dolt's own user in its image) under the image's init, before the
# image's entrypoint.
#
# The application VM's node agent keeps its files in its directory, mounted
# read-only at QD_DOLT_TLS_SOURCE: `ca.crt`, the installation's CA, and
# `machine.pem`, the certificate then the key, renamed whole at each
# renewal. Dolt reads its certificate once, when it starts, and never again
# (tools/verify/dolt_mtls.sh), so it gets a copy of its own, taken here at
# each start: QD_DOLT_TLS_DIR (a tmpfs), 0600, each file written aside and
# renamed. There is no watcher: a renewed certificate reaches Dolt only when
# it restarts, which its node agent does when Dolt is free (`qd-node`,
# QD_DOLT_RESTART). The copy is what Dolt serves until then, so it lasts
# what the application VM's certificates last (a year at most, the role
# `app`'s ceiling).
set -eu
SRC=${QD_DOLT_TLS_SOURCE:-/run/qd-node}
DST=${QD_DOLT_TLS_DIR:-/run/dolt-tls}
WAIT=${QD_DOLT_TLS_WAIT:-300}

log() { echo "dolt-tls: $*" >&2; }

# The passwords from their files when given (the core's package, decision
# 0080): the server makes root from DOLT_ROOT_PASSWORD at its first start,
# and init/10-app-user.sh reads DOLT_APP_PASSWORD.
for name in DOLT_ROOT_PASSWORD DOLT_APP_PASSWORD; do
  file_var="${name}_FILE"
  file=$(eval "printf %s \"\${$file_var:-}\"")
  if [ -n "$file" ]; then
    value=$(cat "$file") || { log "cannot read $file_var"; exit 1; }
    export "$name=$value"
  fi
done

present() { [ -s "$SRC/ca.crt" ] && [ -s "$SRC/machine.pem" ]; }

waited=0
until present; do
  if [ "$waited" -ge "$WAIT" ]; then
    log "no ca.crt and machine.pem in $SRC after ${WAIT}s: is the node agent running?"
    exit 1
  fi
  [ "$waited" = 0 ] && log "waiting for the node agent's files in $SRC"
  sleep 2
  waited=$((waited + 2))
done

mkdir -p "$DST"
chmod 700 "$DST"
for f in ca.crt machine.pem; do
  cp "$SRC/$f" "$DST/.$f.new"
  chmod 600 "$DST/.$f.new"
  mv -f "$DST/.$f.new" "$DST/$f"
done
log "the machine's certificate copied for this start"

# The first start of an empty volume: the image runs its init scripts
# (init/10-app-user.sh) with `dolt sql` against the server it has just
# started, which with a client certificate required refuses them (`dolt
# sql` has none). So the first start is made here: Dolt on the loopback
# alone, without TLS, makes root from DOLT_ROOT_PASSWORD and DOLT_ROOT_HOST
# and runs the init scripts; it is stopped, the image is told its init is
# done, and Dolt starts as configured. Nothing listens beyond the loopback
# meanwhile.
DATA=/var/lib/dolt
CONFIG=/etc/dolt/servercfg.d/config.yaml
if [ ! -f "$DATA/.init_completed" ]; then
  # Never a root without a password.
  [ -n "${DOLT_ROOT_PASSWORD:-}" ] || { log "no DOLT_ROOT_PASSWORD (or _FILE) for the first init"; exit 1; }
  INIT=$(mktemp)
  sed -e 's/^  host: .*/  host: 127.0.0.1/' \
    -e '/^  tls_cert:/d' -e '/^  tls_key:/d' -e '/^  require_secure_transport:/d' \
    -e '/^  ca_cert:/d' -e '/^  require_client_cert:/d' "$CONFIG" >"$INIT"
  (cd "$DATA" && exec dolt sql-server --config="$INIT") >/dev/null 2>&1 &
  pid=$!
  ready=0
  for _ in $(seq 120); do
    if bash -c 'exec 3<>/dev/tcp/127.0.0.1/3306' 2>/dev/null; then ready=1; break; fi
    sleep 1
  done
  [ "$ready" = 1 ] || { log "Dolt did not start for its first init"; exit 1; }
  for f in /docker-entrypoint-initdb.d/*.sh; do
    [ -e "$f" ] || continue
    (cd "$DATA" && bash "$f") || { log "$f failed"; kill "$pid"; exit 1; }
  done
  kill "$pid"
  wait "$pid" 2>/dev/null || true
  rm -f "$INIT"
  touch "$DATA/.init_completed"
  log "first init done"
fi

exec docker-entrypoint.sh "$@"
