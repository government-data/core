#!/bin/sh
# PostgreSQL with the machine's certificate (decision 0066, step 8): the
# entrypoint of the service `postgres` in infra/postgres/tls.yml, run as
# root before the image's own entrypoint.
#
# The application VM's node agent keeps its files in its directory (mounted
# read-only at QD_PG_TLS_SOURCE): `ca.crt`, the installation's CA, and
# `machine.pem`, the certificate then the key, renamed whole at each
# renewal. Its owner and modes are the agent's, and PostgreSQL refuses a key
# that is not its own user's or root's, or that others can read; and the
# agent cannot reach this container's processes. So this side copies the
# two files to QD_PG_TLS_DIR, owned by postgres and 0600 (each written aside
# and renamed), once before PostgreSQL starts and again whenever they
# change (checked every QD_PG_TLS_CHECK_EVERY seconds), then reloads
# PostgreSQL (`pg_ctl reload`), which takes the new certificate for new
# connections without a restart (tools/verify/pg_client_cert.sh). The
# certificate and the key are both read from the one file, so they always
# match.
#
# pg_hba.conf and pg_ident.conf (beside this script, QD_PG_CONF_SOURCE) are
# copied there too at start, so PostgreSQL reads files of its own whatever
# the checkout's modes; a change to them takes a restart.
#
# The watcher runs detached, a child of the container's init (`init: true`
# in tls.yml), never of PostgreSQL: a child the postmaster does not know
# exiting would look to it like a crashed backend.
set -eu
SRC=${QD_PG_TLS_SOURCE:-/run/qd-node}
DST=${QD_PG_TLS_DIR:-/var/lib/postgresql/tls}
CONF=${QD_PG_CONF_SOURCE:-/etc/qd/postgres}
EVERY=${QD_PG_TLS_CHECK_EVERY:-30}

log() { echo "postgres-tls: $*" >&2; }

# put FROM NAME MODE: FROM as DST/NAME, owned by postgres, written aside.
put() {
  cp "$1" "$DST/.$2.new"
  chown postgres:postgres "$DST/.$2.new"
  chmod "$3" "$DST/.$2.new"
  mv -f "$DST/.$2.new" "$DST/$2"
}

present() { [ -s "$SRC/ca.crt" ] && [ -s "$SRC/machine.pem" ]; }

# What the files are now: changes when either does. Never printed.
fingerprint() { cat "$SRC/ca.crt" "$SRC/machine.pem" 2>/dev/null | sha256sum | cut -d' ' -f1; }

install_files() {
  put "$SRC/ca.crt" ca.crt 600
  put "$SRC/machine.pem" machine.pem 600
}

watch() {
  current=$1
  while sleep "$EVERY"; do
    present || continue
    now=$(fingerprint)
    [ "$now" = "$current" ] && continue
    install_files
    if su-exec postgres pg_ctl reload -s -D "$PGDATA"; then
      current=$now
      log "a renewed certificate installed; PostgreSQL reloaded"
    else
      log "a renewed certificate installed; PostgreSQL could not be reloaded, trying again"
    fi
  done
}

# The first start (decision 0066, point 16; the user's ruling of
# 2026-11-13): the application VM's first certificate is signed by `server
# app-machine`, which needs this database, while PostgreSQL with TLS needs
# that certificate. So, while the agent's files are not there, PostgreSQL
# serves on its loopback alone and without TLS (the image's first init
# included), as Dolt's first start does: `listen_addresses=127.0.0.1` and
# pg_hba.first-start.conf (the socket, and the loopback with a password;
# nothing else). The command runs in this container's network namespace
# (app-machine.yml), so nothing it sends leaves it. Once both files are
# there, that PostgreSQL is stopped and the one with TLS started. A start
# with the files there, as every later one, has no such window.
first_start() {
  put "$CONF/pg_hba.first-start.conf" pg_hba.first-start.conf 600
  log "no machine.pem in $SRC yet: PostgreSQL on its loopback alone, without TLS, until it is there"
  docker-entrypoint.sh postgres -c listen_addresses=127.0.0.1 -c ssl=off \
    -c hba_file="$DST/pg_hba.first-start.conf" </dev/null &
  pid=$!
  trap 'su-exec postgres pg_ctl stop -s -m fast -D "$PGDATA"; exit 143' INT TERM
  # Ready on the loopback, so the image's init is over before it is stopped.
  until present && su-exec postgres pg_isready -q -h 127.0.0.1; do
    kill -0 "$pid" 2>/dev/null || { log "PostgreSQL stopped during the first start"; exit 1; }
    sleep 2
  done
  su-exec postgres pg_ctl stop -s -w -m fast -D "$PGDATA"
  wait "$pid" 2>/dev/null || true
  trap - INT TERM
  log "the node agent's files are there: PostgreSQL goes on with TLS"
}

# The grants an existing installation lacks (quality-sdk decision 0009):
# once PostgreSQL answers on its socket, the init script that revokes
# CONNECT from PUBLIC on the core's databases, as a new one runs it. It is
# safe to run twice; it is tried again while PostgreSQL is not ready (the
# image's first init restarts it) and given up after a while.
grants() {
  tries=0
  while [ "$tries" -lt 60 ]; do
    tries=$((tries + 1))
    sleep 5
    su-exec postgres pg_isready -q || continue
    if su-exec postgres bash "$GRANTS" >/dev/null 2>&1; then
      log "CONNECT on the core's databases revoked from PUBLIC"
      return 0
    fi
  done
  log "could not revoke CONNECT from PUBLIC on the core's databases"
}
GRANTS=/docker-entrypoint-initdb.d/40-revoke-public-connect.sh

mkdir -p "$DST"
chown postgres:postgres "$DST"
chmod 700 "$DST"
put "$CONF/pg_hba.conf" pg_hba.conf 600
put "$CONF/pg_ident.conf" pg_ident.conf 600
present || first_start
install_files
log "the machine's certificate installed"
(watch "$(fingerprint)" </dev/null &)
(grants </dev/null &)
exec docker-entrypoint.sh "$@"
