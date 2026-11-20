#!/bin/sh
# The worker's way to the lake (decision 0066, point 10): the entrypoint of
# the service `lake` in infra/worker/lake.yml, a Traefik on the worker's
# loopback, run as the node agent's user (uid 65532).
#
# The lake takes only machines' certificates, at its gate, and DuckDB
# cannot present one. This proxy does it for the worker: it takes DuckDB's
# connection on the loopback, over TLS with the machine's certificate
# (nothing travels unencrypted, point 12; DuckDB verifies it under the
# machine's name, QD_LAKE_URL), and opens one to the gate (QD_LAKE_GATE,
# `<the lake node's name>:<port>`), verified against the installation's CA
# under that name, presenting the same certificate. It is a TCP proxy: it
# reads no request, and the job's signature and STS credentials reach
# RustFS as DuckDB made them.
#
# It writes its one dynamic file, in a tmpfs, from the environment.
# Traefik reads a certificate's file when it reads the configuration that
# names it and watches that file alone, so when the agent renews
# `machine.pem` (looked at every QD_LAKE_CHECK_EVERY seconds) the file is
# written again and Traefik takes the certificate without a restart; the
# connections already open go on.
#
# QD_LAKE_HTTP1_ONLY=1 makes its listener offer HTTP/1.1 alone (ALPN), for
# a client that offers HTTP/2: the gate speaks to this proxy over a
# connection of its own, in HTTP/1.1, so a client that agreed HTTP/2 with
# the listener is not understood (verified with RustFS 1.0.5's replication
# client, tools/verify/rustfs_replication_gate.sh). The source lake's
# replication proxy sets it (infra/rustfs/replication.yml); the worker's
# clients speak HTTP/1.1 and leave it unset, which offers what Traefik
# offers by default.
#
# QD_LAKE_LISTEN_TLS=0 makes its listener plain HTTP, for a lake pool node's
# replication proxy alone: it listens on the loopback of RustFS's own network
# namespace, where every node maps the name qd-lake-replication, and RustFS
# replicates to it as a target that is not `secure`; the hop to the gate is
# TLS with the machine's certificate as always (0066 step 11, ruling 5,
# tools/verify/rustfs_pool_replication.sh). 1, the default, serves TLS.
#
# The watcher is detached (a child of the container's init, `init: true`).
set -eu
SRC=${QD_LAKE_SOURCE:-/run/qd-node}
DYN=${QD_LAKE_DYNAMIC:-/run/qd-lake}
EVERY=${QD_LAKE_CHECK_EVERY:-30}
WAIT=${QD_LAKE_WAIT:-300}
LISTEN=${QD_LAKE_LISTEN:-127.0.0.1:9000}
: "${QD_LAKE_GATE:?set QD_LAKE_GATE to the lake's gate, <the lake node's name>:<port>}"
NAME=${QD_LAKE_GATE%:*}
case ${QD_LAKE_HTTP1_ONLY:-0} in
1) ALPN='
      alpnProtocols: [http/1.1]' ;;
0) ALPN='' ;;
*) echo "lake-proxy: QD_LAKE_HTTP1_ONLY is 0 or 1" >&2 && exit 1 ;;
esac
case ${QD_LAKE_LISTEN_TLS:-1} in
1) LISTEN_TLS='
      tls: {}' ;;
0) LISTEN_TLS='' ;;
*) echo "lake-proxy: QD_LAKE_LISTEN_TLS is 0 or 1" >&2 && exit 1 ;;
esac

log() { echo "lake-proxy: $*" >&2; }
present() { [ -s "$SRC/ca.crt" ] && [ -s "$SRC/machine.pem" ]; }
# What the agent's files are now. Never printed.
fingerprint() { cat "$SRC/ca.crt" "$SRC/machine.pem" 2>/dev/null | sha256sum | cut -d' ' -f1; }

# The dynamic file, aside then renamed.
write() {
  cat >"$DYN/.lake.yml.new" <<YML
tcp:
  routers:
    lake:
      entryPoints: [lake]
      rule: "HostSNI(\`*\`)"$LISTEN_TLS
      service: gate
  services:
    gate:
      loadBalancer:
        serversTransport: gate
        servers:
          - address: "$QD_LAKE_GATE"
            tls: true
  serversTransports:
    gate:
      tls:
        serverName: "$NAME"
        rootCAs: [$SRC/ca.crt]
        certificates:
          - certFile: $SRC/machine.pem
            keyFile: $SRC/machine.pem
tls:
  options:
    default:
      minVersion: VersionTLS12$ALPN
  stores:
    default:
      defaultCertificate:
        certFile: $SRC/machine.pem
        keyFile: $SRC/machine.pem
YML
  mv -f "$DYN/.lake.yml.new" "$DYN/lake.yml"
}

watch() {
  current=$1
  while sleep "$EVERY"; do
    present || continue
    now=$(fingerprint)
    [ "$now" = "$current" ] && continue
    write
    current=$now
    log "the renewed certificate is read again"
  done
}

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
mkdir -p "$DYN"
write
(watch "$(fingerprint)" &)

exec traefik --providers.file.directory="$DYN" --providers.file.watch=true \
  --entrypoints.lake.address="$LISTEN" \
  --entrypoints.lake.transport.respondingTimeouts.readTimeout="${QD_LAKE_READ_TIMEOUT:-0s}" \
  --log.level="${QD_LAKE_LOG_LEVEL:-ERROR}" "$@"
