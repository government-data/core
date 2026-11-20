#!/bin/bash
# Shared by the verification scripts of decision 0073 (grafana_*.sh): one
# host, qd.verify.example, served by Traefik v3.6 with the repository's own
# middlewares (infra/traefik/dynamic/middlewares.yml: app-frame-headers on
# /grafana, api-chain on /api, web-chain on the rest), in front of
# grafana_quality_standins.py (Quality Data's OIDC provider and pages, its
# chart routes) and Grafana 13.2.5 configured as infra/observability's (OAuth
# generic with Quality Data, auto-login, embedding, secure lax cookies, the
# emergency admin from a file) with Infinity installed from a checksummed
# archive and no network: the throwaway network is --internal, so nothing in
# it reaches the Internet. Sourced after machines_lib.sh with NET_OPTS=--internal.
# Synthetic people and rows; throwaway keys and secrets, never printed.
APP=qd.verify.example
INFINITY_VERSION=4.1.6
INFINITY_SHA256=b534f46bdf169068a86f58f5b37f6d6adf603906c6755a66bbaa76ab789442c8
INFINITY_ZIP=${INFINITY_ZIP:-$HOME/.cache/qd-verify/yesoreyeram-infinity-datasource-$INFINITY_VERSION.linux_amd64.zip}
INFINITY_URL="https://grafana.com/api/plugins/yesoreyeram-infinity-datasource/versions/$INFINITY_VERSION/download?os=linux&arch=amd64"
TRAEFIK_IMAGE=traefik:v3.6
PY_IMAGE=python:3.12-slim
GF_ADMIN=qd-emergency-admin

# The archive once, from grafana.com, kept only if its SHA-256 is the pinned
# one; unpacked into $D/plugins, which Grafana mounts read-only.
infinity_plugin() {
  if [ ! -f "$INFINITY_ZIP" ]; then
    mkdir -p "$(dirname "$INFINITY_ZIP")"
    curl -sSfL -m 300 -o "$INFINITY_ZIP.part" "$INFINITY_URL" && mv "$INFINITY_ZIP.part" "$INFINITY_ZIP"
  fi
  expect "Infinity $INFINITY_VERSION archive's SHA-256" "$INFINITY_SHA256" "$(sha256sum "$INFINITY_ZIP" | cut -d' ' -f1)"
  mkdir -p "$D/plugins"
  python3 -I -c 'import sys, zipfile; zipfile.ZipFile(sys.argv[1]).extractall(sys.argv[2])' "$INFINITY_ZIP" "$D/plugins"
  # zipfile keeps no mode bits: the backend must be made executable.
  chmod -R a+rX "$D/plugins" && chmod 755 "$D"/plugins/yesoreyeram-infinity-datasource/gpx_infinity_linux_amd64
}

# Keys, certificates, people and Traefik's files in $D/c; the log in $D/log.
quality_files() {
  ca ca
  cert app ca "/O=qd/CN=$APP" "$APP"
  mkdir -p "$D/c" "$D/log" "$D/dyn" "$D/jar" "$D/prov/datasources" "$D/prov/dashboards" "$D/dash"
  chmod 777 "$D/log" "$D/jar"
  cp "$D/ca.crt" "$D/app.crt" "$D/app.key" "$D/c/"
  openssl rand -hex 32 >"$D/c/jwt.key"
  openssl rand -hex 24 >"$D/c/client_secret"
  openssl rand -hex 16 >"$D/c/gf_admin"
  cp "$ROOT/tools/verify/grafana_quality_standins.py" "$D/c/standins.py"
  # Who is who: ana views one dataset, bruno audits, carla both, root admin, dario nothing.
  cat >"$D/c/people.json" <<'JSON'
{"ana":   {"policy": ["loans-viewer"],             "grafana_orgs": ["quality"],          "datasets": ["loans"]},
 "bruno": {"policy": ["auditor"],                  "grafana_orgs": ["audit"],            "datasets": []},
 "carla": {"policy": ["auditor", "quality-viewer"], "grafana_orgs": ["quality", "audit"], "datasets": ["loans", "savings", "bare"]},
 "root":  {"policy": ["quality-admin"],            "grafana_orgs": ["quality", "audit", "admin"], "datasets": ["loans", "savings", "bare"]},
 "dario": {"policy": [],                           "grafana_orgs": [],                   "datasets": []}}
JSON
  cp "$ROOT/infra/traefik/dynamic/middlewares.yml" "$D/dyn/middlewares.yml"
  cat >"$D/dyn/routes.yml" <<YML
tls:
  certificates: [{ certFile: /c/app.crt, keyFile: /c/app.key }]
http:
  routers:
    grafana: { rule: "Host(\`$APP\`) && PathPrefix(\`/grafana\`)", entryPoints: [websecure], tls: {}, priority: 10, service: grafana, middlewares: [app-frame-headers] }
    oauth:   { rule: "Host(\`$APP\`) && (PathPrefix(\`/api/oauth/\`) || PathPrefix(\`/api/test/\`))", entryPoints: [websecure], tls: {}, priority: 20, service: app, middlewares: [api-chain] }
    api:     { rule: "Host(\`$APP\`) && PathPrefix(\`/api\`) && !PathPrefix(\`/api/internal/machines/\`)", entryPoints: [websecure], tls: {}, priority: 15, service: api, middlewares: [api-chain] }
    web:     { rule: "Host(\`$APP\`)", entryPoints: [websecure], tls: {}, priority: 1, service: app, middlewares: [web-chain] }
  services:
    grafana: { loadBalancer: { servers: [{ url: "http://grafana:3000" }] } }
    app:     { loadBalancer: { servers: [{ url: "http://app:8080" }] } }
    api:     { loadBalancer: { servers: [{ url: "http://api:8080" }] } }
YML
  cat >"$D/c/traefik.yml" <<'YML'
global: { checkNewVersion: false, sendAnonymousUsage: false }
entryPoints: { websecure: { address: ":443" } }
providers: { file: { directory: /dyn } }
log: { level: ERROR }
accessLog: { filePath: /log/access.json, format: json }
YML
  chmod -R a+rX "$D/c" "$D/dyn" "$D/prov" "$D/dash"
}

# The stand-ins (aliases app, api and evil.verify.example) and Traefik ($APP).
quality_edge_up() {
  local role
  for role in app api; do
    docker run -d --name "$P-$role" --network "$NET" --network-alias "$role" \
      $([ "$role" = api ] && echo --network-alias evil.verify.example) \
      -v "$D/c:/c:ro" -v "$D/log:/log" -e KEY_FILE=/c/jwt.key -e PEOPLE_FILE=/c/people.json \
      -e LOG_FILE="/log/$role.jsonl" -e API_HOSTS="$APP,${API_HOST2:-$APP}" -e TOKEN_TTL="${TOKEN_TTL:-86400}" -e CLIENT_SECRET_FILE=/c/client_secret \
      -e REDIRECT_URI="https://$APP/grafana/login/generic_oauth" "$PY_IMAGE" python3 -I /c/standins.py "$role" >/dev/null
  done
  docker run -d --name "$P-traefik" --network "$NET" --network-alias "$APP" \
    $([ -n "${API_HOST2:-}" ] && echo --network-alias "$API_HOST2") -v "$D/c:/c:ro" -v "$D/dyn:/dyn:ro" \
    -v "$D/log:/log" "$TRAEFIK_IMAGE" --configFile=/c/traefik.yml >/dev/null
  TRAEFIK_IP=$(docker inspect -f "{{(index .NetworkSettings.Networks \"$NET\").IPAddress}}" "$P-traefik")
}

# quality_grafana_up NAME [DOCKER OPTION...]: Grafana as infra/observability
# runs it, plus what decision 0073 adds (org mapping, the session's cap).
quality_grafana_up() {
  local name=$1
  shift
  docker run -d --name "$P-$name" --network "$NET" --network-alias "$name" \
    -v "$D/c:/c:ro" -v "$D/prov:/etc/grafana/provisioning:ro" -v "$D/dash:/dash:ro" -v "$D/plugins:/qd-plugins:ro" \
    -e GF_PATHS_PLUGINS=/qd-plugins -e GF_PLUGINS_PREINSTALL_DISABLED=true \
    -e GF_SERVER_ROOT_URL="https://$APP/grafana/" -e GF_SERVER_SERVE_FROM_SUB_PATH=true \
    -e GF_SECURITY_ALLOW_EMBEDDING=true -e GF_SECURITY_COOKIE_SECURE=true -e GF_SECURITY_COOKIE_SAMESITE=lax \
    -e GF_SECURITY_DISABLE_GRAVATAR=true -e GF_SECURITY_ADMIN_USER=$GF_ADMIN \
    -e GF_SECURITY_ADMIN_PASSWORD__FILE=/c/gf_admin -e GF_USERS_ALLOW_SIGN_UP=false -e GF_USERS_DEFAULT_LANGUAGE=es-ES \
    -e GF_AUTH_GENERIC_OAUTH_ENABLED=true -e "GF_AUTH_GENERIC_OAUTH_NAME=Quality Data" \
    -e GF_AUTH_GENERIC_OAUTH_AUTO_LOGIN=true -e GF_AUTH_GENERIC_OAUTH_ALLOW_SIGN_UP=true \
    -e GF_AUTH_GENERIC_OAUTH_CLIENT_ID=grafana -e GF_AUTH_GENERIC_OAUTH_CLIENT_SECRET__FILE=/c/client_secret \
    -e "GF_AUTH_GENERIC_OAUTH_SCOPES=openid email profile" -e GF_AUTH_GENERIC_OAUTH_USE_PKCE=true \
    -e GF_AUTH_GENERIC_OAUTH_AUTH_URL="https://$APP/api/oauth/authorize" \
    -e GF_AUTH_GENERIC_OAUTH_TOKEN_URL="https://$APP/api/oauth/token" \
    -e GF_AUTH_GENERIC_OAUTH_API_URL="https://$APP/api/oauth/userinfo" -e GF_AUTH_GENERIC_OAUTH_TLS_CLIENT_CA=/c/ca.crt \
    -e GF_AUTH_GENERIC_OAUTH_LOGIN_ATTRIBUTE_PATH=email -e GF_AUTH_GENERIC_OAUTH_EMAIL_ATTRIBUTE_PATH=email \
    -e GF_AUTH_GENERIC_OAUTH_NAME_ATTRIBUTE_PATH=name \
    -e "GF_AUTH_GENERIC_OAUTH_ROLE_ATTRIBUTE_PATH=${GF_ROLE_PATH:-contains(not_null(policy, \`[]\`), 'quality-admin') && 'Admin' || length(not_null(grafana_orgs, \`[]\`)) > \`0\` && 'Viewer' || ''}" \
    -e GF_AUTH_GENERIC_OAUTH_ROLE_ATTRIBUTE_STRICT=true -e GF_AUTH_GENERIC_OAUTH_ALLOW_ASSIGN_GRAFANA_ADMIN=false \
    -e GF_AUTH_GENERIC_OAUTH_ORG_ATTRIBUTE_PATH="${GF_ORG_PATH-grafana_orgs}" \
    -e GF_AUTH_GENERIC_OAUTH_ORG_MAPPING="${GF_ORG_MAPPING-quality:2:Viewer audit:1:Viewer admin:1:Admin admin:2:Admin}" \
    -e GF_AUTH_LOGIN_MAXIMUM_LIFETIME_DURATION="${GF_MAX_LIFETIME:-24h}" \
    -e GF_ANALYTICS_REPORTING_ENABLED=false -e GF_ANALYTICS_CHECK_FOR_UPDATES=false \
    -e GF_ANALYTICS_CHECK_FOR_PLUGIN_UPDATES=false -e GF_NEWS_NEWS_FEED_ENABLED=false -e GF_LOG_LEVEL=warn \
    "$@" "$GRAFANA_IMAGE" >/dev/null
  local i
  for i in $(seq 90); do
    [ "$(gfa "$name" /api/health -o /dev/null -w '%{http_code}')" = 200 ] && return 0
    docker ps -q --filter "name=^/$P-$name\$" | grep -q . || break
    sleep 2
  done
  note "Grafana $name not ready: $(docker logs "$P-$name" 2>&1 | tail -3 | cut -c1-400)"
  return 1
}

# gfa NAME PATH [CURL OPTION...]: Grafana NAME's API as its emergency admin.
gfa() {
  local name=$1 path=$2
  shift 2
  docker run --rm --network "$NET" -v "$D/c:/c:ro" --entrypoint sh "$CURL" -c \
    'n=$1; p=$2; shift 2; exec curl -s -m 30 -u "'$GF_ADMIN':$(cat /c/gf_admin)" "$@" "http://$n:3000/grafana$p"' \
    sh "$name" "$path" "$@"
}

# web PERSON URL [CURL OPTION...]: a person's own curl through Traefik, with
# their cookie jar ($D/jar/PERSON): Quality Data's session and Grafana's.
web() {
  local who=$1 url=$2
  shift 2
  docker run --rm --network "$NET" -v "$D/c:/c:ro" -v "$D/jar:/jar" "$CURL" -s -m 60 --cacert /c/ca.crt \
    -b "/jar/$who" -c "/jar/$who" "$@" "$url"
}
# signin PERSON: signed in to Quality Data, then Grafana's auto-login
# followed to its end; prints the last URL Grafana sent the person to.
signin() {
  web "$1" "https://$APP/api/test/signin?person=$1" -o /dev/null
  web "$1" "https://$APP/grafana/login" -L -o /dev/null -w '%{http_code} %{url_effective}'
}
# as PERSON PATH [CURL OPTION...]: Grafana's API with the person's session.
as() {
  local who=$1 path=$2
  shift 2
  web "$who" "https://$APP/grafana$path" "$@"
}
# ds_query PERSON ORG JSON-QUERIES: /api/ds/query in organisation ORG.
ds_query() {
  local who=$1 org=$2 queries=$3
  shift 3
  as "$who" /api/ds/query -X POST -H 'Content-Type: application/json' -H "X-Grafana-Org-Id: $org" \
    -d "{\"from\":\"now-1h\",\"to\":\"now\",\"queries\":$queries}" "$@"
}
# api_log FILTER: the stand-in API's log lines (jq-less: python) matching a
# Python expression over the entry e, one JSON per line.
api_log() {
  python3 -I -c 'import json,sys
for l in open(sys.argv[1]):
    e = json.loads(l)
    if eval(sys.argv[2], {"e": e}): print(l, end="")' "$D/log/${2:-api}.jsonl" "$1" 2>/dev/null
}

# quality_stack [DOCKER OPTION...]: the edge, Grafana, the organisation
# Quality made by the API (id 2: the open-source build provisions no
# organisation, V3), then its one data source (no base URL, V4) and the
# dashboards of $D/dash/quality provisioned and read again.
quality_stack() {
  quality_edge_up
  quality_grafana_up grafana "$@" || return 1
  gfa grafana /api/orgs -X POST -H 'Content-Type: application/json' -d '{"name":"Quality"}' -o /dev/null
  cat >"$D/prov/datasources/quality.yml" <<YML
apiVersion: 1
datasources:
  - name: Quality API
    uid: quality-api
    orgId: 2
    type: yesoreyeram-infinity-datasource
    isDefault: true
    jsonData:
      auth_method: oauthPassThru
      oauthPassThru: true
      allowedHosts: ["https://$APP/api/quality/"]
      tlsAuthWithCACert: true
    secureJsonData:
      tlsCACert: \$__file{/c/ca.crt}
YML
  printf 'apiVersion: 1\nproviders:\n  - { name: quality, orgId: 2, type: file, disableDeletion: true, allowUiUpdates: false, options: { path: /dash/quality } }\n' \
    >"$D/prov/dashboards/quality.yml"
  chmod -R a+rX "$D/prov" "$D/dash"
  gfa grafana /api/admin/provisioning/datasources/reload -X POST -o /dev/null
  gfa grafana /api/admin/provisioning/dashboards/reload -X POST -o /dev/null
}
# quality_dash LANG [--meets]: the generated qd-quality-LANG as step 6 would
# make it (grafana_quality_dashboard.py), into $D/dash/quality.
quality_dash() {
  local dir=$ROOT/infra/observability/grafana/dashboards
  local src="$dir/Quality Data · Quality indicators/quality.json"
  [ "$1" = es ] && src="$dir/Quality Data · Indicadores de calidad/quality.json"
  mkdir -p "$D/dash/quality"
  python3 -I "$ROOT/tools/verify/grafana_quality_dashboard.py" "$src" "$ROOT/crates/quality/charts" \
    "https://$APP/api/quality/" "$D/dash/quality/$1.json" "${@:2}" >/dev/null
}

# browse STEPS-FILE: grafana_browser.py's scenario in this machine's
# Chromium, a fresh profile each time unless PROFILE names one; its JSON out.
browse() {
  local spki
  spki=$(openssl x509 -in "$D/app.crt" -pubkey -noout | openssl pkey -pubin -outform der | openssl dgst -sha256 -binary | base64)
  python3 -I "$ROOT/tools/verify/grafana_browser.py" "$APP" "$TRAEFIK_IP" "$spki" "${PROFILE:-$D/chrome-$RANDOM}" "$1"
}
# The browser is stopped by its PID with the rest.
eval "containers_cleanup() $(declare -f cleanup | tail -n +2)"
cleanup() {
  local f
  for f in "$D"/chrome-*.pid "$D"/profile*.pid; do [ -f "$f" ] && kill "$(cat "$f")" 2>/dev/null; done
  containers_cleanup
}
