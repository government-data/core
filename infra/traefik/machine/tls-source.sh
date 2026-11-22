#!/bin/sh
# The package's Traefik static configuration by the web entry point's
# certificate source (decision 0087), written at each start by
# machine/entrypoint.sh before Traefik runs:
#
#   tls-source.sh SOURCE TARGET
#
# SOURCE is the version's traefik.yml (development reads it as it is);
# TARGET is what Traefik runs (the package's --configFile). By QD_TLS_MODE:
#
#   file  no ACME: `certificatesResolvers` removed and `websecure`'s default
#         TLS without a resolver (`tls: {}`), so every router is served the
#         default certificate, certs/local.crt (dynamic/tls.yml);
#   acme  Let's Encrypt as in SOURCE, its `email` ACME_EMAIL: Traefik with
#         --configFile reads no static configuration from its environment.
#
# Anything else, or `acme` without an address, stops with a message, and so
# does a SOURCE whose shape it does not know: Traefik does not start.
set -eu
SOURCE=$1
TARGET=$2
MODE=${QD_TLS_MODE:-file}

die() { echo "tls-source: $*" >&2; exit 1; }

mkdir -p "$(dirname "$TARGET")"
case $MODE in
file)
  awk '
    /^certificatesResolvers:/ { skip = 1; next }
    skip && /^[A-Za-z]/ { skip = 0 }
    skip { next }
    held != "" {
      if ($0 == "        certResolver: letsencrypt") { print "      tls: {}"; held = ""; next }
      print held; held = ""
    }
    $0 == "      tls:" { held = $0; next }
    { print }
  ' "$SOURCE" >"$TARGET.new"
  if grep -v '^ *#' "$TARGET.new" | grep -qE 'certResolver|certificatesResolvers|acme:' || [ "$(grep -c '^      tls: {}$' "$TARGET.new")" != 1 ]; then
    die "$SOURCE is not in the shape this script knows: no file mode"
  fi
  ;;
acme)
  EMAIL=${ACME_EMAIL:-}
  printf '%s' "$EMAIL" | grep -qE '^[^@[:space:]"\\|&]+@[^@[:space:]"\\|&]+\.[^@[:space:]"\\|&]+$' ||
    die "QD_TLS_MODE=acme needs ACME_EMAIL, an e-mail address (Let's Encrypt's contact)"
  [ "$(grep -c '^      email: ' "$SOURCE")" = 1 ] && grep -q '^certificatesResolvers:' "$SOURCE" ||
    die "$SOURCE is not in the shape this script knows: no ACME mode"
  sed "s|^      email: .*|      email: \"$EMAIL\"|" "$SOURCE" >"$TARGET.new"
  ;;
*) die "QD_TLS_MODE is file or acme, not '$MODE'" ;;
esac
mv -f "$TARGET.new" "$TARGET"
echo "tls-source: the web's certificate source is $MODE" >&2
