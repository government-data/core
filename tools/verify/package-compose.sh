#!/bin/bash
# Verification of how Docker Compose runs a module's package the way the
# update agent of quality-sdk runs it (decision 0080, step 2), before the
# core's package compose is written:
#
#   docker compose --project-name <module> --project-directory <dir>/current \
#     --file <dir>/current/compose.yml --env-file <dir>/.env \
#     up --detach --no-build --remove-orphans --wait --wait-timeout <s>
#
# with no profile and no second file. A throwaway package with the shape the
# core's will have (a root compose.yml that includes files under infra/, and
# overrides their services) is extracted under a temporary /opt-like folder
# with a `current` link; the per-installation files (.env, secrets/, certs/)
# live beside the version folders and are reached through QD_MODULE_DIR.
# It shows:
#
#   - QD_MODULE_DIR is required: without it `config` stops with its message;
#   - the .env beside the version folders is the one read, also by the
#     included files, and a .env inside the version folder is not;
#   - every service without a profile starts and is healthy, one with a
#     profile does not, and `build:` sections (a context with no Dockerfile,
#     a context that does not exist) do not stop `up --no-build`;
#   - the including file's mount on the same target replaces the included
#     file's (Traefik's certificates come from QD_MODULE_DIR/certs), the
#     PostgreSQL password comes from QD_MODULE_DIR/secrets as a secret, and
#     the included files' relative paths are the version folder's;
#   - a folder of QD_MODULE_DIR mounted inside the package's Traefik dynamic
#     folder is read by Traefik's file provider (a router of its own), and
#     whether its mount point must be in the package, and whether a file
#     put in it after Traefik started is read; and the other way round, the
#     QD_MODULE_DIR folder watched with the package's inside it (reported);
#   - what happens when `current` moves to a new version whose files differ
#     but whose compose is the same: the bind mounts' sources, and whether
#     `up` recreates the containers (reported, not asserted).
#
# Images: traefik:v3.6, postgres:16-alpine, traefik/whoami:v1.11 and
# alpine:3, local or public; nothing from a private registry. Everything it
# makes is named qs-verify-pkg… and removed on exit, also on failure. Run
# alone (no build running), from the repository root:
#
#   bash tools/verify/package-compose.sh
#
# Needs docker (with Compose v2 or later), curl and openssl.
set -u
export LC_ALL=C
# Compose's file secrets and binds keep the host's modes: PostgreSQL reads
# its password file as its own user (uid 70), so the folders must let it in.
umask 022
TRAEFIK=${TRAEFIK_IMAGE:-traefik:v3.6}
POSTGRES=${POSTGRES_IMAGE:-postgres:16-alpine}
WHOAMI=${WHOAMI_IMAGE:-traefik/whoami:v1.11}
ALPINE=${ALPINE_IMAGE:-alpine:3}
PORT=${QS_VERIFY_PORT:-18443}
P=qs-verify-pkg
D=$(mktemp -d)
chmod 755 "$D"
MOD=$D/opt/quality-data/$P
FAILED=0
ok() { printf '[OK  ] %s\n' "$*"; }
ko() { printf '[FAIL] %s\n' "$*"; FAILED=1; }
found() { printf '[INFO] %s\n' "$*"; }
expect() { if [ "$2" = "$3" ]; then ok "$1: $3"; else ko "$1: expected '$2', got '$3'"; fi; }
dc() { # the agent's command line, then ARGS
  docker compose --project-name "$P" --project-directory "$MOD/current" \
    --file "$MOD/current/compose.yml" --env-file "${ENV_FILE:-$MOD/.env}" "$@"
}
cleanup() {
  dc down --volumes --remove-orphans --timeout 5 >/dev/null 2>&1
  docker rm -f "$P-psql" "$P-nested" "$P-swapped" >/dev/null 2>&1
  rm -rf -- "${D:?}"
}
trap cleanup EXIT
echo "Docker $(docker version --format '{{.Server.Version}}'), $(docker compose version)"

# --- The package, version $1 --------------------------------------------------
package() {
  local v=$MOD/$1
  mkdir -p "$v/infra/edge/dynamic" "$v/infra/db/initdb"
  # The mount point of QD_MODULE_DIR/traefik: a read-only bind mount cannot
  # get a new folder for a mount inside it (checked in 3).
  [ "${2:-}" = bare ] || mkdir -p "$v/infra/edge/dynamic/installation"
  cat >"$v/compose.yml" <<'EOF'
include:
  - infra/edge/docker-compose.yml
  - infra/db/docker-compose.yml
secrets:
  db_password:
    file: ${QD_MODULE_DIR:?set QD_MODULE_DIR to the module's directory}/secrets/db_password
services:
  edge:
    volumes:
      - ${QD_MODULE_DIR:?set QD_MODULE_DIR to the module's directory}/certs:/etc/traefik/certs:ro
      - ${QD_MODULE_DIR:?set QD_MODULE_DIR to the module's directory}/traefik:/etc/traefik/dynamic/installation:ro
  db:
    environment:
      POSTGRES_PASSWORD: ""
      POSTGRES_PASSWORD_FILE: /run/secrets/db_password
    secrets: [db_password]
  app:
    image: ${IMAGE_APP:?set IMAGE_APP}
    build:
      context: ./no-such-context
    command: ["sleep", "infinity"]
    init: true
    healthcheck:
      test: ["CMD", "true"]
      interval: 2s
EOF
  cat >"$v/infra/edge/docker-compose.yml" <<'EOF'
services:
  edge:
    image: ${IMAGE_EDGE:-traefik:v3.6}
    command: ["--configFile=/etc/traefik/traefik.yml"]
    environment:
      ACME_EMAIL: ${ACME_EMAIL:?set ACME_EMAIL in your environment}
    ports:
      - "127.0.0.1:${QS_VERIFY_PORT:-18443}:443"
    volumes:
      - ./traefik.yml:/etc/traefik/traefik.yml:ro
      - ./dynamic:/etc/traefik/dynamic:ro
      - ./certs:/etc/traefik/certs:ro
    healthcheck:
      test: ["CMD", "traefik", "healthcheck", "--ping"]
      interval: 2s
      retries: 30
  web:
    image: ${IMAGE_WEB:-traefik/whoami:v1.11}
    build:
      context: ../..
      dockerfile: infra/docker/web.Dockerfile
  oneshot:
    image: ${IMAGE_APP:?set IMAGE_APP}
    profiles: ["oneshot"]
    command: ["true"]
EOF
  cat >"$v/infra/edge/traefik.yml" <<'EOF'
entryPoints:
  websecure:
    address: ":443"
  traefik:
    address: ":8080"
ping: {}
providers:
  file:
    directory: /etc/traefik/dynamic
    watch: true
EOF
  cat >"$v/infra/edge/dynamic/routes.yml" <<EOF
tls:
  stores:
    default:
      defaultCertificate:
        certFile: /etc/traefik/certs/local.crt
        keyFile: /etc/traefik/certs/local.key
http:
  routers:
    web:
      rule: "Host(\`localhost\`)"
      entryPoints: [websecure]
      tls: {}
      service: web
      middlewares: [version]
  middlewares:
    version:
      headers:
        customResponseHeaders:
          X-Package-Version: "$1"
  services:
    web:
      loadBalancer:
        servers:
          - url: "http://web:80"
EOF
  cat >"$v/infra/db/docker-compose.yml" <<'EOF'
volumes:
  pgdata:
services:
  db:
    image: ${IMAGE_DB:-postgres:16-alpine}
    environment:
      POSTGRES_PASSWORD: ${DB_PASSWORD:?set DB_PASSWORD in your environment}
    volumes:
      - pgdata:/var/lib/postgresql/data
      - ./initdb:/docker-entrypoint-initdb.d:ro
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U postgres"]
      interval: 2s
      retries: 30
EOF
  printf 'CREATE TABLE package_marker (version text);\nINSERT INTO package_marker VALUES (%s);\n' \
    "'$1'" >"$v/infra/db/initdb/10-marker.sql"
  # A .env inside the version folder (the project directory): never read.
  printf 'ACME_EMAIL=\nQD_MODULE_DIR=/nonexistent\n' >"$v/.env"
}

# --- The per-installation files, beside the version folders ------------------
mkdir -p "$MOD/secrets" "$MOD/certs" "$MOD/traefik"
package 1.0.0
ln -s 1.0.0 "$MOD/current"
SECRET=$(openssl rand -hex 16)
(umask 077 && printf '%s' "$SECRET" >"$MOD/secrets/db_password")
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 \
  -subj "/CN=qs-verify-module-dir" -addext "subjectAltName=DNS:localhost,DNS:installation.localhost" \
  -keyout "$MOD/certs/local.key" -out "$MOD/certs/local.crt" 2>/dev/null
chmod 644 "$MOD/certs/local.key" "$MOD/secrets/db_password"
cat >"$MOD/traefik/installation.yml" <<'EOF'
http:
  routers:
    installation:
      rule: "Host(`installation.localhost`)"
      entryPoints: [websecure]
      tls: {}
      service: web
EOF
cat >"$MOD/.env" <<EOF
QD_MODULE_DIR=$MOD
IMAGE_EDGE=$TRAEFIK
IMAGE_DB=$POSTGRES
IMAGE_WEB=$WHOAMI
IMAGE_APP=$ALPINE
ACME_EMAIL=verify@example.invalid
DB_PASSWORD=not-the-secret
QS_VERIFY_PORT=$PORT
EOF
grep -v '^QD_MODULE_DIR=' "$MOD/.env" >"$D/no-module-dir.env"

# --- 1. QD_MODULE_DIR is required ---------------------------------------------
if out=$(ENV_FILE=$D/no-module-dir.env dc config 2>&1); then
  ko "config without QD_MODULE_DIR passed"
else
  case $out in
    *"set QD_MODULE_DIR to the module's directory"*) ok "config without QD_MODULE_DIR stops with its message" ;;
    *) ko "config without QD_MODULE_DIR: $out" ;;
  esac
fi

# --- 2. The configuration as the agent's command line resolves it ------------
cfg=$(dc config 2>&1) || ko "config: $cfg"
case $cfg in *"verify@example.invalid"*) ok "an included file reads the .env beside the versions" ;;
  *) ko "the included file's ACME_EMAIL is not the .env's" ;; esac
case $cfg in *nonexistent*) ko "the .env inside the version folder was read" ;;
  *) ok "the .env inside the version folder is not read" ;; esac
expect "services in the configuration (no profile)" "app db edge web" \
  "$(dc config --services 2>/dev/null | sort | tr '\n' ' ' | sed 's/ $//')"
found "bind sources in the configuration:"
printf '%s\n' "$cfg" | grep -E '^\s+source: /' | sort -u | sed 's/^ */         /'

# --- 3. Up, as the agent runs it ---------------------------------------------
package 0.9.0 bare
if docker run --rm --name "$P-nested" -v "$MOD/0.9.0/infra/edge/dynamic:/d:ro" \
  -v "$MOD/traefik:/d/installation:ro" "$ALPINE" true >/dev/null 2>&1; then
  found "a mount inside a read-only bind mount gets its mount point made"
else
  found "a mount inside a read-only bind mount needs its mount point in the package"
fi
before=$(docker images -q | sort -u | wc -l)
if out=$(dc up --detach --no-build --remove-orphans --wait --wait-timeout 120 2>&1); then
  ok "up --no-build --wait with build: sections (no Dockerfile, no context)"
else
  ko "up: $(printf '%s' "$out" | tail -n 3)"
  dc logs --no-log-prefix db 2>&1 | tail -n 5
fi
expect "no image built" "$before" "$(docker images -q | sort -u | wc -l)"
expect "running services" "app db edge web" \
  "$(dc ps --services --status running 2>/dev/null | sort | tr '\n' ' ' | sed 's/ $//')"
expect "the profiled one-shot is not created" "" "$(dc ps --all --services 2>/dev/null | grep oneshot)"

# --- 4. Secrets, certificates and files from where they belong ----------------
docker run --rm --name "$P-psql" --network "${P}_default" "$POSTGRES" \
  psql "postgresql://postgres:$SECRET@db/postgres" -Atc 'SELECT version FROM package_marker' \
  >"$D/marker" 2>&1
expect "PostgreSQL's password is QD_MODULE_DIR/secrets', the initdb the version's" "1.0.0" \
  "$(cat "$D/marker")"
if docker run --rm --name "$P-psql" --network "${P}_default" "$POSTGRES" \
  psql "postgresql://postgres:not-the-secret@db/postgres" -Atc 'SELECT 1' >/dev/null 2>&1; then
  ko "the .env's DB_PASSWORD still signs in"
else
  ok "the .env's DB_PASSWORD is replaced by the secret"
fi
cn=$(openssl s_client -connect "127.0.0.1:$PORT" -servername localhost </dev/null 2>/dev/null |
  openssl x509 -noout -subject 2>/dev/null)
expect "Traefik's certificate (QD_MODULE_DIR/certs)" "subject=CN=qs-verify-module-dir" "$cn"
curl() { command curl -sk --max-time 5 "$@"; }
web() { curl -o /dev/null -w '%{http_code}' --resolve "$1:$PORT:127.0.0.1" "https://$1:$PORT/"; }
for _ in $(seq 20); do [ "$(web installation.localhost)" = 200 ] && break; sleep 1; done
expect "the package's route" 200 "$(web localhost)"
expect "a route from QD_MODULE_DIR mounted in the dynamic folder" 200 "$(web installation.localhost)"
# A file put in the nested folder after Traefik started (as a renewal or the
# installer's internal.yml would be), aside then renamed; then a change at
# the top of the watched folder.
late() { printf 'http:\n  routers:\n    late:\n      rule: "Host(`late.localhost`)"\n      entryPoints: [websecure]\n      tls: {}\n      service: web\n' >"$MOD/traefik/.late.yml.new" &&
  mv -f "$MOD/traefik/.late.yml.new" "$MOD/traefik/late.yml"; }
late
sleep 5
found "a file renamed into the nested folder after start: $(web late.localhost) (200 read, 404 not)"
touch "$MOD/current/infra/edge/dynamic/routes.yml"
sleep 5
found "the same after a change at the top of the watched folder: $(web late.localhost)"
# The other way round: QD_MODULE_DIR's folder is the watched one, the
# package's dynamic folder a read-only folder inside it.
rm -f "$MOD/traefik/late.yml"
docker run -d --name "$P-swapped" --network "${P}_default" -p "127.0.0.1:$((PORT + 1)):443" \
  -v "$MOD/current/infra/edge/traefik.yml:/etc/traefik/traefik.yml:ro" -v "$MOD/certs:/etc/traefik/certs:ro" \
  -v "$MOD/traefik:/etc/traefik/dynamic" -v "$MOD/current/infra/edge/dynamic:/etc/traefik/dynamic/package:ro" \
  "$TRAEFIK" --configFile=/etc/traefik/traefik.yml >/dev/null
swapped() { curl -o /dev/null -w '%{http_code}' --resolve "$1:$((PORT + 1)):127.0.0.1" "https://$1:$((PORT + 1))/"; }
for _ in $(seq 20); do [ "$(swapped localhost)" = 200 ] && break; sleep 1; done
found "swapped: the package's route from the nested folder: $(swapped localhost)"
found "swapped: the nested mount point made in QD_MODULE_DIR: $([ -d "$MOD/traefik/package" ] && echo yes || echo no)"
late
sleep 5
found "swapped: a file renamed into the watched folder after start: $(swapped late.localhost)"
docker rm -f "$P-swapped" >/dev/null 2>&1
version() { curl -D - -o /dev/null --resolve "localhost:$PORT:127.0.0.1" "https://localhost:$PORT/" |
  tr -d '\r' | sed -n 's/^X-Package-Version: //Ip'; }
expect "the package's dynamic files are version 1.0.0's" 1.0.0 "$(version)"
found "bind sources of the running Traefik:"
docker inspect "$(dc ps -q edge)" --format '{{range .Mounts}}{{if eq .Type "bind"}}{{.Source}}{{"\n"}}{{end}}{{end}}' |
  sed '/^$/d; s/^/         /'

# --- 5. current moves to 1.0.1 (same compose, other files) ---------------------
id=$(dc ps -q edge)
package 1.0.1
ln -sfn 1.0.1 "$MOD/current"
out=$(dc up --detach --no-build --remove-orphans --wait --wait-timeout 120 2>&1) || ko "up 1.0.1: $out"
if [ "$(dc ps -q edge)" = "$id" ]; then found "after the link moved, up did not recreate Traefik"; else
  found "after the link moved, up recreated Traefik"; fi
sleep 3
found "the package version Traefik serves after up: '$(version)'"
dc restart edge >/dev/null 2>&1
dc up --detach --no-build --wait --wait-timeout 60 >/dev/null 2>&1
found "the package version Traefik serves after a restart: '$(version)'"
out=$(dc up --detach --no-build --force-recreate --wait --wait-timeout 120 2>&1) || ko "force-recreate: $out"
expect "after --force-recreate, the new version's files" 1.0.1 "$(version)"

[ "$FAILED" = 0 ] && echo "All checks passed." || echo "Some checks failed."
exit "$FAILED"
