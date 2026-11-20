#!/bin/bash
# How long `docker stop` takes on a lake pool node's Alloy under the loop of
# infra/rustfs/pool/docker-compose.yml (decision 0071, step 4; the user's
# ruling of 2026-11-15), the loop read from the compose file as Compose runs
# it: Alloy's image, its user, read-only, /tmp on tmpfs, no init, in another
# container's network namespace as in RustFS's. Alloy runs a configuration
# of one line, as nothing here is pushed or received; what is measured is
# the loop's trap. Alloy alone first (the control), then RUNS times under
# the loop: how long docker stop takes, Alloy's state and exit code (137
# when Docker's kill ended it after its 10 s), and that it stays stopped.
# The same loop then ends Alloy when its namespace goes (with KILL, the
# user's ruling of 2026-11-16, where the trap sends TERM), as a check that
# what is measured is the loop that works.
#
# Throwaway containers only (qd-verify-alloystop-…), no port published.
# Run from the repository root:
#
#   bash tools/verify/alloy_loop_stop.sh
#
# Needs docker and uv.
set -u
export LC_ALL=C
ALLOY=${ALLOY_IMAGE:-grafana/alloy:v1.21.3}
P=qd-verify-alloystop
RUNS=${QD_VERIFY_RUNS:-3}
D=$(mktemp -d)
chmod 755 "$D"
cleanup() {
  docker ps -aq --filter "name=^$P-" | xargs -r docker rm -f >/dev/null 2>&1
  docker network rm "$P" >/dev/null 2>&1
  rm -rf "$D"
}
trap cleanup EXIT
FAILED=0
ok() { printf '[OK  ] %s\n' "$*"; }
ko() {
  printf '[FAIL] %s\n' "$*"
  FAILED=1
}
expect() { if [ "$2" = "$3" ]; then ok "$1: $3"; else ko "$1: expected '$2', got '$3'"; fi; }

# The service's entrypoint as Compose runs it ($$ is its escape of $).
mapfile -d '' LOOP < <(uv run -q --no-project --with pyyaml==6.0.2 python3 -I -c '
import sys, yaml
e = yaml.safe_load(open(sys.argv[1]))["services"]["alloy"]["entrypoint"]
sys.stdout.write("".join(x.replace("$$", "$") + "\0" for x in e))' infra/rustfs/pool/docker-compose.yml)
expect "the compose file's Alloy: /bin/sh -c, a loop on /proc/net/dev around Alloy" "/bin/sh -c 1 1" \
  "${LOOP[0]} ${LOOP[1]} $(grep -c '^/bin/alloy "$@" &$' <<<"${LOOP[2]}") $(grep -c 'grep -c : /proc/net/dev' <<<"${LOOP[2]}")"
echo "   (its trap: $(grep -o "trap '[^']*'" <<<"${LOOP[2]}"))"
# The user's ruling of 2026-11-16: docker stop (the trap) sends TERM, so that
# Alloy flushes; the namespace gone, the loop sends KILL, as Alloy's flush
# would wait its fixed minute for a Mimir it cannot reach.
expect "the trap sends TERM, the namespace's branch KILL" "TERM KILL" \
  "$(grep -o "trap 'kill -[A-Z]*" <<<"${LOOP[2]}" | sed 's/.*-//') $(sed -n '/proc\/net\/dev/,/exit 1/p' <<<"${LOOP[2]}" | grep -o 'kill -[A-Z]*' | sed 's/.*-//')"
printf 'logging {\n  level = "info"\n}\n' >"$D/one.alloy"
chmod 644 "$D/one.alloy"
docker network create "$P" >/dev/null || exit 1
ns() { docker run -d --name "$P-ns" --network "$P" alpine:3.20 sleep 3600 >/dev/null; }
alloy() { # POLICY [ENTRYPOINT...]: Alloy under the loop, or under ENTRYPOINT
  local e=("${LOOP[@]}")
  [ $# -gt 1 ] && e=("${@:2}")
  docker rm -f "$P-alloy" >/dev/null 2>&1
  docker run -d --name "$P-alloy" --network "container:$P-ns" --user 10001:10001 --read-only --tmpfs /tmp \
    --restart "$1" -v "$D/one.alloy:/etc/qd-alloy/one.alloy:ro" --entrypoint "${e[0]}" "$ALLOY" \
    "${e[@]:1}" run --server.http.listen-addr=127.0.0.1:12345 --storage.path=/tmp/alloy /etc/qd-alloy >/dev/null
}
stopping() { # LABEL: docker stop of Alloy, its time, state and exit code
  local start took state
  start=$(date +%s%N)
  docker stop "$P-alloy" >/dev/null
  took=$((($(date +%s%N) - start) / 1000000))
  state=$(docker inspect -f '{{.State.Status}} {{.State.ExitCode}}' "$P-alloy")
  expect "$1: docker stop ends it, and it stays stopped" exited "${state%% *}"
  expect "$1: ... within 5 s, before Docker's kill" yes "$([ "$took" -lt 5000 ] && echo yes || echo no)"
  echo "   ($1: stopped in $took ms; state and exit code: $state; Alloy's last line:" \
    "$(docker logs "$P-alloy" 2>&1 | tail -1 | cut -c1-120))"
}
ready() { # Alloy's own server answers in the namespace, within 60 s
  for _ in $(seq 60); do
    docker run --rm --network "container:$P-ns" alpine:3.20 wget -q -O /dev/null http://127.0.0.1:12345/-/ready 2>/dev/null &&
      echo ready && return
    sleep 1
  done
  echo "not ready"
}
ns
alloy no /bin/alloy
expect "the control, Alloy alone, answers" ready "$(ready)"
stopping "the control"
for i in $(seq "$RUNS"); do
  alloy unless-stopped
  expect "run $i: Alloy under the loop answers" ready "$(ready)"
  stopping "run $i"
done

echo "== the loop still ends Alloy when its namespace goes"
alloy on-failure:3
expect "Alloy under the loop answers" ready "$(ready)"
docker restart "$P-ns" >/dev/null
back=''
for s in $(seq 60); do
  [ "$(docker inspect -f '{{.RestartCount}}' "$P-alloy")" -ge 1 ] && back=$s && break
  sleep 1
done
expect "the loop ended it and Docker started it again" yes "${back:+yes}"
expect "... saying why" 1 "$(docker logs "$P-alloy" 2>&1 | grep -c '^qd: RustFS restarted')"
echo "   (within ${back:-no restart in 60} s)"

echo
if [ "$FAILED" = 0 ]; then echo "ALL OK"; else
  echo "SOME CHECKS FAILED"
  exit 1
fi
