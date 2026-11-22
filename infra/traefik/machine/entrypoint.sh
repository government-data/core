#!/bin/sh
# The application VM's certificate in its Traefik (decision 0066, points 10,
# 13 and 15): the entrypoint of the service `traefik` in
# infra/traefik/certificate.yml. Traefik runs as root, so it reads the node
# agent's files as they are (uid 65532, the key 0600).
#
# It copies the agent's `machine.pem` (the certificate then the key) into
# Traefik's watched directory as `app-machine.pem`, aside then renamed, the
# key and the certificate in one file: the internal entry point serves it
# (INTERNAL_CERT_FILE and INTERNAL_KEY_FILE, internal.yml) and the
# serversTransport `lake` presents it to the lake's gate. Traefik reads a
# certificate's file when it reads its configuration, and reads that again
# when a file of its watched directory changes, so when the agent renews
# `machine.pem` (looked at every QD_TRAEFIK_CHECK_EVERY seconds) the copy
# is written again and Traefik serves and presents the renewed one without
# a restart; the connections already open go on.
#
# It does not wait for the agent: Traefik is the people's entry point too,
# and starts at once; until the first copy only the machines' entry points
# fail their handshake. The watcher is detached (a child of the container's
# init, `init: true`).
set -eu
SRC=${QD_TRAEFIK_SOURCE:-/run/qd-node}
DYN=${QD_TRAEFIK_DYNAMIC:-/etc/traefik/dynamic}
EVERY=${QD_TRAEFIK_CHECK_EVERY:-30}

log() { echo "app-machine: $*" >&2; }

# The package (QD_TLS_MODE set) runs the static configuration of its web
# certificate source (decision 0087), written first: if it cannot be, Traefik
# does not start.
if [ -n "${QD_TLS_MODE:-}" ]; then
  /etc/qd/traefik/tls-source.sh /etc/traefik/traefik.yml /run/qd-traefik/traefik.yml
fi
# What the agent's certificate is now. Never printed.
fingerprint() { sha256sum "$SRC/machine.pem" 2>/dev/null | cut -d' ' -f1; }

# The copy, aside then renamed; a name Traefik's file provider does not load.
copy() {
  [ -s "$SRC/machine.pem" ] || return 1
  (umask 077 && cp "$SRC/machine.pem" "$DYN/.app-machine.pem.new") &&
    mv -f "$DYN/.app-machine.pem.new" "$DYN/app-machine.pem"
}

watch() {
  current=$1
  while sleep "$EVERY"; do
    now=$(fingerprint) || continue
    [ -n "$now" ] && [ "$now" != "$current" ] || continue
    if copy; then
      current=$now
      log "machine.pem copied (first or renewed); Traefik reads it again"
    else
      log "could not copy machine.pem, trying again"
    fi
  done
}

first=""
if copy; then
  first=$(fingerprint)
else
  log "no machine.pem in $SRC yet: is the node agent running? Copied once it is there"
fi
(watch "$first" </dev/null &)

exec /entrypoint.sh "$@"
