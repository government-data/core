#!/bin/bash
# Verification of how an application module's route reaches Traefik in an
# installation (quality-sdk decision 0002, "Amendment to steps 3, 7 and 9",
# points 2 to 4), with the core package's layout:
#
#   /etc/traefik/dynamic          QD_MODULE_DIR/internal (read-write), the
#                                 watched folder, where the update agent
#                                 writes module-<module>.yml
#   /etc/traefik/dynamic/package  the package's infra/traefik/dynamic (ro)
#
# A file put in a subfolder after start is not read (core decision 0081),
# so the routes go in the watched folder itself. A route file with the
# shape the agent writes (the same template header as apps.yml,
# `module-chain@file` then `module-forward-auth@file`) is renamed into it
# after Traefik started, and removed again. It shows:
#
#   - with no route file, /docs/ is not served (404);
#   - a route file renamed into the watched folder after start is read by
#     the file provider (hot reload), a `.tmp` file beside it is not, and
#     its removal takes the route away;
#   - `module-forward-auth` asks the gate at http://api:8080/auth/verify:
#     without a session a page goes to the sign-in with `next`, a request
#     that is not a page gets 401, with a session the module answers and
#     receives the gate's X-Auth-* headers, not the ones the client sent;
#   - `module-chain`'s headers (HSTS, nosniff, X-Frame-Options SAMEORIGIN)
#     and no CSP from the core;
#   - `/docs` alone and `/docsx` (the route matches `/docs` and `/docs/…`
#     only).
#
# The gate is a stand-in (python:3.12-slim) answering as /auth/verify does
# (crates/auth-gateway/src/http/verify.rs; its behaviour is tested against
# a real Postgres in crates/api/tests/verify_it.rs); the module is
# traefik/whoami, which echoes the headers it gets. Images: traefik:v3.6,
# traefik/whoami:v1.11, python:3.12-slim, local or public. Everything it
# makes is named qs-verify-route… and removed on exit, also on failure.
# Run alone (no build running), from the repository root:
#
#   tools/verify/module_route.sh
#
# Port: 127.0.0.1:${QS_VERIFY_PORT:-18443} only.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PORT="${QS_VERIFY_PORT:-18443}"
NAME=qs-verify-route
NET="$NAME-net"
TMP="$(mktemp -d)"
FAILED=0

cleanup() {
  docker rm -f "$NAME-traefik" "$NAME-api" "$NAME-docs" >/dev/null 2>&1 || true
  docker network rm "$NET" >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT INT TERM

pass() { echo "PASS  $*"; }
fail() { echo "FAIL  $*"; FAILED=1; }

mkdir -p "$TMP/internal" "$TMP/certs" "$TMP/static"
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 1 \
  -subj "/CN=localhost" -keyout "$TMP/certs/local.key" -out "$TMP/certs/local.crt" \
  >/dev/null 2>&1
chmod 0644 "$TMP/certs/local.key"
# The containers run as root; the folders as the test made them.

cat >"$TMP/static/traefik.yml" <<'EOF'
global: { checkNewVersion: false, sendAnonymousUsage: false }
entryPoints:
  websecure: { address: ":443" }
  traefik: { address: ":8080" }
ping: {}
providers:
  file: { directory: /etc/traefik/dynamic, watch: true }
EOF

# The gate: /auth/verify's answers, by the stand-in's rules.
cat >"$TMP/gate.py" <<'EOF'
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import quote
class Gate(BaseHTTPRequestHandler):
    def do_GET(self):
        if not self.path.startswith("/auth/verify"):
            self.send_response(404); self.end_headers(); return
        if "qd_session=ok" in (self.headers.get("Cookie") or ""):
            self.send_response(204)
            for h, v in [("X-Auth-User", "user-1"), ("X-Auth-Email", "a@example.test"),
                         ("X-Auth-Role", "admin"), ("X-Auth-Org", "")]:
                self.send_header(h, v)
        elif "text/html" in (self.headers.get("Accept") or ""):
            self.send_response(302)
            uri = self.headers.get("X-Forwarded-Uri") or "/"
            # Absolute, from the forwarded origin, as the real gate's
            # public_origin: Traefik resolves a relative one against the
            # gate's own address.
            origin = "%s://%s" % (self.headers.get("X-Forwarded-Proto") or "https",
                                  self.headers.get("X-Forwarded-Host") or "")
            self.send_header("Location", origin + "/login?next=" + quote(uri, safe="/") + "&page=1")
        else:
            self.send_response(401)
        self.end_headers()
    def log_message(self, *a): pass
HTTPServer(("0.0.0.0", 8080), Gate).serve_forever()
EOF

docker network create "$NET" >/dev/null
docker run -d --name "$NAME-api" --network "$NET" --network-alias api \
  -v "$TMP/gate.py:/gate.py:ro" python:3.12-slim python /gate.py >/dev/null
docker run -d --name "$NAME-docs" --network "$NET" --network-alias docs-web \
  traefik/whoami:v1.11 --port 8080 >/dev/null
docker run -d --name "$NAME-traefik" --network "$NET" \
  -p "127.0.0.1:$PORT:443" \
  -e APP_DOMAIN=localhost -e QD_TLS_MODE=file \
  -v "$TMP/static/traefik.yml:/etc/traefik/traefik.yml:ro" \
  -v "$TMP/internal:/etc/traefik/dynamic" \
  -v "$ROOT/infra/traefik/dynamic:/etc/traefik/dynamic/package:ro" \
  -v "$TMP/certs:/etc/traefik/certs:ro" \
  traefik:v3.6 --configFile=/etc/traefik/traefik.yml >/dev/null

for _ in $(seq 1 30); do
  docker exec "$NAME-traefik" traefik healthcheck --ping >/dev/null 2>&1 && break
  sleep 1
done

URL="https://localhost:$PORT"
code() { curl -sk -o /dev/null -w '%{http_code}' "$@"; }
# Waits until $1 answers $2 (the file provider reloads within seconds).
until_code() {
  local path="$1" want="$2" got=""
  shift 2
  for _ in $(seq 1 20); do
    got="$(code "$@" "$URL$path")"
    [ "$got" = "$want" ] && return 0
    sleep 1
  done
  echo "  last answer: $got" >&2
  return 1
}

HTML=(-H "Accept: text/html")
SESSION=(-H "Cookie: qd_session=ok")

if until_code /docs/ 404 "${HTML[@]}" "${SESSION[@]}"; then
  pass "no route file: /docs/ is not served (404)"
else
  fail "no route file: /docs/ should be 404"
fi

cat >"$TMP/internal/module-docs.yml.tmp" <<'EOF'
# Written by quality-data-agent from the module's manifest; do not edit.
{{ $domain := env "APP_DOMAIN" | default "localhost" }}
{{ $resolver := "letsencrypt" }}{{ if eq (env "QD_TLS_MODE") "file" }}{{ $resolver = "" }}{{ end }}
http:
  routers:
    module-docs:
      rule: "Host(`{{ $domain }}`) && (Path(`/docs`) || PathPrefix(`/docs/`))"
      entryPoints: [websecure]
      tls:
        certResolver: "{{ $resolver }}"
      priority: 50
      service: module-docs
      middlewares: [module-chain@file, module-forward-auth@file]
  services:
    module-docs:
      loadBalancer:
        servers:
          - url: "http://docs-web:8080"
EOF
mv "$TMP/internal/module-docs.yml.tmp" "$TMP/internal/module-docs.yml"

if until_code /docs/ 200 "${HTML[@]}" "${SESSION[@]}"; then
  pass "route file put after start: /docs/ served with a session (200)"
else
  fail "route file put after start: /docs/ should be 200 with a session"
  docker logs "$NAME-traefik" 2>&1 | grep -v "^$" | tail -15 | sed "s/^/  log: /"
fi

loc="$(curl -sk -o /dev/null -w '%{redirect_url}' "${HTML[@]}" "$URL/docs/guide/?q=1")"
if [ "$(code "${HTML[@]}" "$URL/docs/guide/?q=1")" = 302 ] &&
  [ "$loc" = "https://localhost:$PORT/login?next=/docs/guide/%3Fq%3D1&page=1" ]; then
  pass "a page without a session goes to sign in and back ($loc)"
else
  fail "a page without a session: expected 302 to /login?next=…, got $loc"
fi

if [ "$(code "$URL/docs/_astro/a.js")" = 401 ]; then
  pass "a request that is not a page, without a session: 401"
else
  fail "a request that is not a page, without a session: expected 401"
fi

body="$(curl -sk "${SESSION[@]}" -H "X-Auth-User: someone-else" "$URL/docs/")"
if grep -q "^X-Auth-User: user-1" <<<"$body" && ! grep -q "someone-else" <<<"$body"; then
  pass "the module gets the gate's X-Auth-User, not the client's"
else
  fail "X-Auth-User: expected the gate's value only"
fi

headers="$(curl -sk -D - -o /dev/null "${SESSION[@]}" "$URL/docs/")"
if grep -qi "^strict-transport-security:" <<<"$headers" &&
  grep -qi "^x-content-type-options: nosniff" <<<"$headers" &&
  grep -qi "^x-frame-options: SAMEORIGIN" <<<"$headers" &&
  ! grep -qi "^content-security-policy:" <<<"$headers"; then
  pass "module-chain: HSTS, nosniff, SAMEORIGIN, no CSP from the core"
else
  fail "module-chain headers"; echo "$headers"
fi

if [ "$(code "${SESSION[@]}" "$URL/docs")" = 200 ] &&
  [ "$(code "${SESSION[@]}" "$URL/docsx")" = 404 ]; then
  pass "/docs is routed, /docsx is not"
else
  fail "/docs and /docsx"
fi

rm "$TMP/internal/module-docs.yml"
if until_code /docs/ 404 "${HTML[@]}" "${SESSION[@]}"; then
  pass "route file removed: /docs/ is no longer served (404)"
else
  fail "route file removed: /docs/ should be 404"
fi

if [ "$FAILED" = 0 ]; then echo "Result: PASS"; else echo "Result: FAIL"; exit 1; fi
