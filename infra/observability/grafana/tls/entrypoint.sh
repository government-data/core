#!/bin/sh
# Grafana with the application VM's machine certificate (decision 0066,
# point 12): the entrypoint of `grafana` in infra/observability/
# certificate.yml, run as root, which then starts Grafana as its own user
# (472), as the image does.
#
# Grafana reads its datasources' certificates only when it provisions them,
# at its start, and the agent's files are 0600 of the agent's user. So at
# each start this copies the agent's `ca.crt` and `machine.pem` (the
# certificate then the key) for Grafana's user into a tmpfs, and makes its
# provisioning from the repository's there:
#   - the Loki and Mimir datasources (datasources/loki.yml, mimir.yml)
#     present the machine's certificate and verify the server against the
#     installation's CA;
#   - the PostgreSQL one (datasources/postgres.yml) verifies PostgreSQL
#     against the same CA (verify-full, its server name GRAFANA_PG_HOST);
#   - the alerting webhooks (alerting/contact-points.yml) present the
#     machine's certificate to the API and verify it against the CA.
# A renewed certificate is taken when Grafana is restarted: the application
# VM's node agent restarts it after each renewal (QD_AFTER_RENEW).
set -eu
SRC=${QD_GRAFANA_SOURCE:-/run/qd-node}
DIR=${QD_GRAFANA_TLS:-/run/qd-grafana}
FROM=${QD_GRAFANA_PROVISIONING:-/etc/grafana/provisioning}
WAIT=${QD_GRAFANA_WAIT:-300}
GRAFANA_UID=${QD_GRAFANA_UID:-472}
GRAFANA_USER=${QD_GRAFANA_USER:-grafana}

log() { echo "grafana-tls: $*" >&2; }
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

# The files, for Grafana's user alone, each aside and then renamed.
mkdir -p "$DIR"
install -m 0644 -o "$GRAFANA_UID" -g 0 "$SRC/ca.crt" "$DIR/.ca.crt.new"
install -m 0600 -o "$GRAFANA_UID" -g 0 "$SRC/machine.pem" "$DIR/.machine.pem.new"
mv -f "$DIR/.ca.crt.new" "$DIR/ca.crt"
mv -f "$DIR/.machine.pem.new" "$DIR/machine.pem"

# The provisioning, the repository's with TLS added (GF_PATHS_PROVISIONING).
P=$DIR/provisioning
rm -rf "$P"
cp -R "$FROM" "$P"
# loki.yml and mimir.yml each have one datasource and end inside it: the
# TLS settings go first in its jsonData, and its secureJsonData after
# everything else.
for name in loki mimir; do
  L=$P/datasources/$name.yml
  awk '{ print } /^    jsonData:$/ && !done { print "      tlsAuth: true"; print "      tlsAuthWithCACert: true"; done = 1 }' \
    "$L" >"$L.new"
  cat >>"$L.new" <<YML
    secureJsonData:
      tlsCACert: \$__file{$DIR/ca.crt}
      tlsClientCert: \$__file{$DIR/machine.pem}
      tlsClientKey: \$__file{$DIR/machine.pem}
YML
  mv -f "$L.new" "$L"
  grep -q '^      tlsAuth: true$' "$L" || { log "datasources/$name.yml: no jsonData to add TLS to"; exit 1; }
done
sed -i "s#^\(      sslRootCertFile:\) .*#\1 $DIR/ca.crt#" "$P/datasources/postgres.yml"
grep -q "sslRootCertFile: $DIR/ca.crt" "$P/datasources/postgres.yml" ||
  { log "datasources/postgres.yml: no sslRootCertFile"; exit 1; }
# Every webhook of the alerting contact points (alerting/contact-points.yml,
# generated: each webhook's settings end with its httpMethod) posts to the
# API's internal entry point with the same certificate, verifying the API
# against the CA (decision 0072, point 10). Grafana keeps the three in its
# database and reads the files again only at its next start.
C=$P/alerting/contact-points.yml
if [ -f "$C" ]; then
  awk -v dir="$DIR" '{ print } /^          httpMethod: "POST"$/ {
      print "          tlsConfig:"
      print "            caCertificate: $__file{" dir "/ca.crt}"
      print "            clientCertificate: $__file{" dir "/machine.pem}"
      print "            clientKey: $__file{" dir "/machine.pem}"
    }' "$C" >"$C.new"
  mv -f "$C.new" "$C"
  [ "$(grep -c '^          tlsConfig:$' "$C")" = "$(grep -c '^        type: "webhook"$' "$C")" ] ||
    { log "alerting/contact-points.yml: a webhook without its TLS"; exit 1; }
fi
chown -R "$GRAFANA_UID:0" "$P"
log "the machine's certificate copied, the datasources' and webhooks' TLS set"

# The image's own start, as Grafana's user, with this environment (busybox's
# su: -p keeps it; its setpriv cannot change the user).
exec su -p -s /bin/sh "$GRAFANA_USER" -c 'exec /run.sh "$@"' grafana "$@"
