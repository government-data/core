#!/bin/bash
# Verification of the join command and infra/join/join.sh against a real
# API (decision 0029; 0066 step 11 and the user's ruling of 2026-11-13: one
# curl link, checked by hashes, whose script brings the server what it
# needs, the compose file only against the join token). With this
# checkout's `server` on a throwaway PostgreSQL made by
# infra/postgres/initdb, the CA of `server ca-init`, and a Traefik 3.6 in
# front for TLS as `https://qd.example` (its certificate made here, trusted
# by the machine as a public one would be through CURL_CA_BUNDLE), the
# administrator asks for a worker's join token and runs the command the API
# answers, as given, on a "machine": a curl 8.11 container (busybox) as
# root, its host name worker-7. It shows: the command runs and leaves the
# CA, the compose file (this version's), a 0600 environment and the token
# 0600 for the agent's user; run again it is the same; the token still
# joins, as the agent will; a changed script fails `sha256sum -c` and runs
# nothing; a wrong --ca-sha256 stops before the token is sent (no request
# for the compose file reaches Traefik); a wrong or used token gets 401 and
# changes nothing; another role is refused; the pinned form of 0029 (`-k
# --pinnedpubkey`, and `--pin` to the script) works, and a wrong pin is
# curl's exit 90. The machine has no Docker, so the script runs with
# QD_JOIN_NO_START=1: the worker's containers are not started here (their
# Alloy pushing its metrics is tools/verify/metrics_join_worker.sh's). The
# installation is tools/verify/join_installation.sh's.
#
# Containers qd-verify-join-pg, -front and the machines' (removed as they
# end), network qd-verify-join-net, port 127.0.0.1:59050 (PostgreSQL); the
# API listens on that network's gateway address, port 59051. Nothing of the
# dev stack is touched. The token and the passwords are never printed: the
# command is written to a 0600 file and run from it. States only. Run from
# the repository root, with the server built (cargo build -p server):
#
#   bash tools/verify/join_script.sh        (SERVER=target/debug/server)
#
# Needs docker, curl, openssl 3 and python3.
set -u
export LC_ALL=C
P=qd-verify-join
PGPORT=59050
APIPORT=59051
MACHINE_IMAGE=curlimages/curl:8.11.1
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
SERVER=${SERVER:-$ROOT/target/debug/server}
[ -x "$SERVER" ] || { echo "no server at $SERVER: cargo build -p server"; exit 1; }
"$SERVER" openapi 2>/dev/null | grep -q '/install/{role}/compose.yml' ||
  { echo "$SERVER has no install routes: build it again"; exit 1; }
D=$(mktemp -d)
chmod 755 "$D"
FAILED=0
mkdir -m 700 "$D/secret"
# Where the joined machines' metrics go (0071 step 4): in the command.
API_ENV=MACHINES_METRICS_PUSH_URL=https://qd-app:9009/api/v1/push
cleanup() {
  install_cleanup
  # The machines' directories hold files of root and of uid 65532.
  docker run --rm --user 0 --entrypoint sh -v "$D:/d" "$MACHINE_IMAGE" -c 'rm -rf /d/m' >/dev/null 2>&1
  rm -rf "$D"
}
trap cleanup EXIT
. "$ROOT/tools/verify/join_installation.sh"
issue worker w7
field "$D/secret/w7.json" install command >"$D/secret/w7.sh"
expect "the answer's command downloads join.sh from the public address" "https://qd.example/api/install/join.sh" \
  "$(field "$D/secret/w7.json" install script_url)"
expect "its script hash is the served join.sh's" "$(sha256sum "$ROOT/infra/join/join.sh" | cut -d' ' -f1)" \
  "$(field "$D/secret/w7.json" install script_sha256)"
expect "its CA hash is the one ca-init printed (the DER's)" \
  "$(openssl x509 -in "$D/ca.crt" -outform DER | sha256sum | cut -d' ' -f1)" "$(field "$D/secret/w7.json" install ca_sha256)"

# machine DIR SCRIPT [ENV...]: SCRIPT (a file in secret/) run as root by sh in a fresh
# machine (worker-7) whose working directory is m/DIR; its exit status.
mkdir -p "$D/m"
machine() {
  local dir=$1 script=$2
  shift 2
  mkdir -p "$D/m/$dir"
  # qd-app, the observability server of --metrics-url, as this server's DNS would give it.
  docker run --rm --network "$NET" --hostname worker-7 --add-host qd-app:192.0.2.9 --user 0 -w "/m/$dir" -v "$D/m:/m" -v "$D/secret:/s:ro" \
    -v "$D/dynamic/front.crt:/front.crt:ro" -e QD_JOIN_NO_START=1 -e QD_JOIN_DIR="/m/$dir/qd" "$@" \
    --entrypoint sh "$MACHINE_IMAGE" "/s/$script" >"$D/m/$dir.out" 2>&1
}
trusted() { machine "$@" -e CURL_CA_BUNDLE=/front.crt; }
# inside DIR COMMAND: COMMAND run in DIR's machine directory, as root.
inside() { docker run --rm --user 0 -v "$D/m:/m" --entrypoint sh "$MACHINE_IMAGE" -c "cd /m/$1/qd 2>/dev/null && $2"; }
compose_requests() { docker logs "$FRONT" 2>/dev/null | grep -c '/api/install/worker/compose.yml'; }

echo "== The command as generated"
trusted run w7.sh
expect "it runs to the end" 0 $?
expect "what it said" "qd-join.sh: OK|qd-join: the installation's CA checked|qd-join: the worker compose file and its files checked|qd-join: written to /m/run/qd as worker-7|qd-join: not started (QD_JOIN_NO_START=1)" \
  "$(paste -sd'|' "$D/m/run.out")"
expect "the files, their modes and owners" "644 0:0 ca.crt|644 0:0 compose.yml|644 0:0 metrics.yml|644 0:0 worker.alloy|600 0:0 .env|700 65532:65532 join|600 65532:65532 join/token" \
  "$(inside run 'stat -c "%a %u:%g %n" ca.crt compose.yml metrics.yml worker.alloy .env join join/token' | paste -sd'|')"
expect "the compose file is this version's worker compose" "$(sha256sum "$ROOT/infra/worker/docker-compose.yml" | cut -d' ' -f1)" \
  "$(inside run 'sha256sum compose.yml' | cut -d' ' -f1)"
expect "metrics.yml gives Alloy the observability server's address as DNS gave it" '- "qd-app:192.0.2.9"' \
  "$(inside run 'grep -- "- " metrics.yml' | sed 's/^ *//')"
expect "and its Alloy this version's worker.alloy" "$(sha256sum "$ROOT/infra/observability/alloy/worker.alloy" | cut -d' ' -f1)" \
  "$(inside run 'sha256sum worker.alloy' | cut -d' ' -f1)"
der() { openssl x509 -outform DER | sha256sum | cut -d' ' -f1; }
expect "the CA is the installation's (its DER)" "$(der <"$D/ca.crt")" "$(inside run 'cat ca.crt' | der)"
expect "the command names where the metrics go" 1 "$(grep -c -- "--metrics-url 'https://qd-app:9009/api/v1/push'" "$D/secret/w7.sh")"
expect "the environment's keys" "QD_MACHINE_NAME=worker-7|QD_CA_SHA256|QD_PUBLIC_URL=https://qd.example/api|APP_DOMAIN=qd.example|QD_JOIN_TOKEN_DIR=/m/run/qd/join|OBS_VM=worker-7|METRICS_PUSH_URL=https://qd-app:9009/api/v1/push|COMPOSE_PROFILES=metrics" \
  "$(inside run 'cat .env' | sed 's/^QD_CA_SHA256=.*/QD_CA_SHA256/' | paste -sd'|')"
token_sha=$(field "$D/secret/w7.json" token | tr -d '\n' | sha256sum | cut -d' ' -f1)
# Compared, never printed (not even its hash, which is what join_tokens keeps).
same=different
[ "$token_sha" = "$(inside run 'sha256sum join/token' | cut -d' ' -f1)" ] && same=same
expect "the token file holds the token, nothing else" same "$same"
inside run 'echo INTERNAL_DOMAIN=qd-app >>.env'
trusted run w7.sh
expect "run again: the same files, and a line it does not own kept" "0 INTERNAL_DOMAIN=qd-app" \
  "$? $(inside run 'grep INTERNAL_DOMAIN .env' | paste -sd'|')"
expect "the machine has not joined: the token is unused" "0" "$(sql 'select count(*) from machines')"

echo "== What stops it"
# The command with one part changed; the token stays in the files under secret/.
sed '1s/ \\$/ \&\& echo "# changed" >>qd-join.sh \\/' "$D/secret/w7.sh" >"$D/secret/changed.sh"
trusted changed changed.sh
expect "a changed script: sha256sum -c fails and nothing runs" "1 FAILED no files" \
  "$? $(grep -o 'FAILED' "$D/m/changed.out" | head -1) $(inside changed 'echo files' || echo no files)"
before=$(compose_requests)
sed "s/--ca-sha256 '[0-9a-f]*'/--ca-sha256 '$(printf '0%.0s' $(seq 64))'/" "$D/secret/w7.sh" >"$D/secret/wrongca.sh"
trusted wrongca wrongca.sh
expect "a wrong --ca-sha256: stopped" "1 stopped, the token was not sent" "$? $(grep -o 'stopped, the token was not sent' "$D/m/wrongca.out")"
expect "and no request for the compose file reached the installation" "$before" "$(compose_requests)"
sed "s/--token '[^']*'/--token 'qdj_$(openssl rand -hex 32 | cut -c1-43)'/" "$D/secret/w7.sh" >"$D/secret/unknown.sh"
trusted unknown unknown.sh
expect "an unknown token: 401, nothing written" "1 unknown, used or expired no files" \
  "$? $(grep -o 'unknown, used or expired' "$D/m/unknown.out") $(inside unknown 'echo files' || echo no files)"
sed 's/--role worker/--role trino-worker/' "$D/secret/w7.sh" >"$D/secret/trino.sh"
trusted trino trino.sh
expect "another role: refused by the script" "1 only worker" "$? $(grep -o 'only worker' "$D/m/trino.out")"
sed 's/--role worker/--role lake/' "$D/secret/w7.sh" >"$D/secret/lake.sh"
trusted lake lake.sh
expect "a pool's node without its pool and name: refused by the script" "1 needs --pool and --name" \
  "$? $(grep -o 'needs --pool and --name' "$D/m/lake.out")"

echo "== The agent joins with the token the script left"
inside run 'cat join/token' >"$D/secret/token"
openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "/CN=worker-7" -keyout "$D/secret/w7.key" -out "$D/secret/w7.csr" 2>/dev/null
python3 -c 'import sys,json;print(json.dumps({"token":open(sys.argv[1]).read(),"name":"worker-7","csr":open(sys.argv[2]).read()}))' \
  "$D/secret/token" "$D/secret/w7.csr" >"$D/secret/join.json"
expect "POST /install/join with it: joined, pending" 201 \
  "$(curl -s -o /dev/null -w '%{http_code}' -H 'content-type: application/json' -d @"$D/secret/join.json" "$API/install/join")"
expect "the machine" "worker-7|worker|default|pending" "$(sql "select name || '|' || role || '|' || pool || '|' || state from machines")"
state() { inside run 'stat -c "%n %Y %s %a" ca.crt compose.yml .env join/token; cat .env' | sha256sum; }
stamp=$(state)
trusted run w7.sh
expect "the command again, its token used: 401, the files unchanged" "1 unknown, used or expired $stamp" \
  "$? $(grep -o 'unknown, used or expired' "$D/m/run.out") $(state)"

echo "== The pinned form of 0029"
pin=$(openssl x509 -in "$D/dynamic/front.crt" -pubkey -noout | openssl pkey -pubin -outform der | openssl dgst -sha256 -binary | base64)
issue worker w8
field "$D/secret/w8.json" install command |
  sed "1s|curl -fsS |curl -fsSk --pinnedpubkey 'sha256//$pin' |; s|--token |--pin 'sha256//$pin' --token |" >"$D/secret/pinned.sh"
machine pinned pinned.sh
expect "-k with the right pin, and --pin to the script, with no CA trusted" "0 written to" "$? $(grep -o 'written to' "$D/m/pinned.out")"
sed "s|sha256//$pin|sha256//$(openssl rand -base64 32)|g" "$D/secret/pinned.sh" >"$D/secret/wrongpin.sh"
machine wrongpin wrongpin.sh
expect "a wrong pin: curl's exit 90, nothing written" "90 no files" "$? $(inside wrongpin 'echo files' || echo no files)"

[ $FAILED = 0 ] && echo "ALL OK" || echo "SOME FAILED"
exit $FAILED
