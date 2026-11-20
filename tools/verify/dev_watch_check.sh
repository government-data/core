#!/bin/sh
# Whether watch mode (`task dev:watch`) answers: the API, the web and the
# docs, each on its own port and through Traefik. From the repository root:
#   tools/verify/dev_watch_check.sh      (or `task dev:check`)
# Without a session, https://localhost/docs must redirect to the sign-in
# (302), not fail (5xx): Traefik asks the API, then the docs' server on port
# 4321 (infra/traefik/watch.yml). Each request waits 5 seconds at most, so a
# server that holds a port and answers nothing fails here instead of hanging.
# API_PORT, WEB_PORT, DOCS_PORT and APP_DOMAIN change where it looks.
set -u
domain=${APP_DOMAIN:-localhost}
api=${API_PORT:-8080}
web=${WEB_PORT:-5173}
docs=${DOCS_PORT:-4321}
fails=0
ok() { echo "[OK  ] $1"; }
ko() { echo "[FAIL] $1"; fails=$((fails + 1)); }
# The status code and the redirect of a GET; 000 when nothing answered.
status() { curl -sk -m 5 -o /dev/null -w '%{http_code} %{redirect_url}' "$1" 2>/dev/null || true; }
# expect NAME URL CODE [REDIRECT PREFIX]
expect() {
  got=$(status "$2")
  code=${got%% *}
  where=${got#* }
  case "$code" in
    "$3") ;;
    000 | '') ko "$1: $2 answered nothing"; return ;;
    *) ko "$1: $2 answered $code, not $3"; return ;;
  esac
  case "$where" in
    "${4:-}"*) ok "$1: $2 $code${4:+ → $where}" ;;
    *) ko "$1: $2 redirects to ${where:-nothing}, not ${4}…" ;;
  esac
}

expect "api" "http://127.0.0.1:$api/health" 200
expect "web" "http://127.0.0.1:$web/" 200
expect "docs" "http://127.0.0.1:$docs/docs/" 200
expect "api via traefik" "https://$domain/api/health" 200
expect "web via traefik" "https://$domain/" 200
expect "docs via traefik, no session" "https://$domain/docs/" 302 "https://$domain/login"

if [ "$fails" != 0 ]; then
  echo "$fails check(s) failed"
  exit 1
fi
echo "all checks passed"
