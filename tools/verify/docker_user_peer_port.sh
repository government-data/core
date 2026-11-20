#!/bin/bash
# How Docker's DOCKER-USER chain keeps a published port to a set of
# addresses (decision 0066 step 11, ruling 2 of 2026-11-14 on a lake pool's
# peers: join.sh writes Docker's firewall rules from the peers' resolved
# addresses so that RustFS's peer port, 9010, published on the server, is
# open to the pool's nodes alone). Verified before join.sh writes them.
#
# Each "server" is a Docker in Docker container (privileged, so with a
# network namespace and a netfilter of its own: nothing is written to this
# host's firewall). On it a container publishes 9010 as the pool's compose
# file does (0.0.0.0:9010:9010, restart unless-stopped), and two containers
# on an internal network of its own talk on 9010 as the gate and RustFS do.
# Outside it, on the verify network, a "peer" and an "outsider" connect to
# the server's address. For each Docker version (27.5.1, what the throwaway
# servers of join_lake_pool.sh run, and 29.4.1) it shows:
#
#   (a) the iptables backend in use and whether Docker's DOCKER-USER chain
#       is there, jumped to from FORWARD;
#   (b) the rules as join.sh would write them: a chain of their own,
#       QD-LAKE-PEERS (a RETURN per peer address, then DROP), filled as a
#       set by one iptables-restore --noflush, and one jump to it at the top
#       of DOCKER-USER for new connections whose original destination port
#       is 9010 and that Docker's DNAT took (a published port);
#       before them both clients connect, after them the peer alone, the
#       containers on the server's own network still talk on 9010, and a
#       rule of DOCKER-USER written by someone else is left as it was;
#   (c) a second run: the same rules, nothing added;
#   (d) the peers changed: the stale address gone, the new one let in;
#   (e) Docker restarted (the daemon alone, the namespace kept): whether
#       the rules are kept;
#   (f) a reboot (the server's namespace made anew): whether anything is
#       kept, and whether rules written before Docker starts, as a unit
#       ordered before docker.service would, survive Docker's start;
#   (g) IPv6: whether a port published on 0.0.0.0 answers on the server's
#       IPv6 address at all (another port of the server answering there);
#   (h) Docker 29 with --firewall-backend=nftables: whether DOCKER-USER is
#       there.
#
# Throwaway containers and network only (qd-verify-fw-…). Run from the
# repository root:
#
#   bash tools/verify/docker_user_peer_port.sh
#
# Needs docker; docker:27.5.1-dind, docker:29.4.1-dind, alpine:3.20 and
# traefik/whoami:v1.11 (pulled if missing), and the network for Alpine's
# nftables package (h).
set -u
export LC_ALL=C
P=qd-verify-fw
NET=$P-net
SUB=${QD_VERIFY_SUBNET:-10.231.85}
SUB6=${QD_VERIFY_SUBNET6:-fd00:85:85::}
VERSIONS=${QD_VERIFY_DOCKER:-27.5.1 29.4.1}
WHOAMI=traefik/whoami:v1.11
FAILED=0
ok() { printf '[OK  ] %s\n' "$*"; }
ko() {
  printf '[FAIL] %s\n' "$*"
  FAILED=1
}
note() { printf '[NOTE] %s\n' "$*"; }
expect() { if [ "$2" = "$3" ]; then ok "$1: $3"; else ko "$1: expected '$2', got '$3'"; fi; }
cleanup() {
  docker ps -aq --filter "name=^$P-" | xargs -r docker rm -f -v >/dev/null 2>&1
  docker network rm "$NET" >/dev/null 2>&1
}
trap cleanup EXIT
cleanup
for image in alpine:3.20 "$WHOAMI"; do
  docker image inspect "$image" >/dev/null 2>&1 || docker pull -q "$image" >/dev/null || exit 1
done
docker network create --ipv6 --subnet "$SUB.0/24" --subnet "${SUB6}/64" "$NET" >/dev/null || exit 1
docker run -d --name "$P-peer" --network "$NET" --ip "$SUB.20" alpine:3.20 sleep 1d >/dev/null
docker run -d --name "$P-outsider" --network "$NET" --ip "$SUB.30" alpine:3.20 sleep 1d >/dev/null
PEER=$SUB.20 OUTSIDER=$SUB.30

# reach CLIENT ADDRESS: whether CLIENT opens a connection to ADDRESS:9010.
reach() { docker exec "$P-$1" sh -c "nc -z -w 3 $2 9010 && echo open || echo refused"; }
# The rules as join.sh writes them, for the addresses given.
rules() {
  {
    printf '*filter\n:QD-LAKE-PEERS - [0:0]\n-F QD-LAKE-PEERS\n'
    for a in "$@"; do printf -- '-A QD-LAKE-PEERS -s %s/32 -j RETURN\n' "$a"; done
    printf -- '-A QD-LAKE-PEERS -j DROP\nCOMMIT\n'
  } >"$D/rules"
  docker exec -i "$M" iptables-restore --noflush <"$D/rules" || return 1
  docker exec "$M" sh -c "$JUMP_CHECK || $JUMP_ADD"
}
JUMP='DOCKER-USER -p tcp -m conntrack --ctstate DNAT --ctorigdstport 9010 --ctdir ORIGINAL -j QD-LAKE-PEERS'
JUMP_CHECK="iptables -C $JUMP 2>/dev/null"
JUMP_ADD="iptables -I $JUMP"
D=$(mktemp -d)
trap 'cleanup; rm -rf "$D"' EXIT
# dockerd on the server, started by hand so it can be restarted alone.
dockerd_start() {
  docker exec -d "$M" sh -c "dockerd-entrypoint.sh dockerd $* >/var/log/dockerd.log 2>&1"
  for _ in $(seq 60); do docker exec "$M" docker info >/dev/null 2>&1 && return 0; sleep 1; done
  return 1
}
dockerd_stop() {
  docker exec "$M" sh -c 'kill $(cat /var/run/docker.pid) 2>/dev/null; for _ in $(seq 30); do [ -e /var/run/docker.pid ] || exit 0; sleep 1; done'
}
server() { # VERSION: a server with dockerd started and the port published
  M=$P-server
  docker rm -f -v "$M" >/dev/null 2>&1
  docker run -d --name "$M" --privileged --network "$NET" --ip "$SUB.10" --ip6 "${SUB6}10" \
    -e DOCKER_TLS_CERTDIR= --entrypoint sh "docker:$1-dind" -c 'sleep 1d' >/dev/null || return 1
  dockerd_start "${2:-}" || return 1
  docker save "$WHOAMI" alpine:3.20 | docker exec -i "$M" docker load -q >/dev/null
}
publish() {
  docker exec "$M" sh -c "docker rm -f rustfs srv cli >/dev/null 2>&1; docker network rm lake >/dev/null 2>&1
    docker run -d --name rustfs --restart unless-stopped -p 0.0.0.0:9010:9010 $WHOAMI --port 9010 >/dev/null &&
    docker network create --internal lake >/dev/null &&
    docker run -d --name srv --restart unless-stopped --network lake $WHOAMI --port 9010 >/dev/null &&
    docker run -d --name cli --restart unless-stopped --network lake alpine:3.20 sleep 1d >/dev/null"
  sleep 2
}
inside() { docker exec "$M" docker exec cli sh -c 'nc -z -w 3 srv 9010 && echo open || echo refused'; }
ours() { docker exec "$M" sh -c 'iptables -S QD-LAKE-PEERS 2>/dev/null; iptables -S DOCKER-USER 2>/dev/null | grep -c -- "-j QD-LAKE-PEERS"'; }
foreign() { docker exec "$M" sh -c 'iptables -S DOCKER-USER | grep -c "192.0.2.99/32 -j DROP"'; }

for v in $VERSIONS; do
  echo "== Docker $v"
  server "$v" || { ko "Docker $v did not start"; continue; }
  publish
  note "(a) iptables: $(docker exec "$M" iptables -V); dockerd's: $(docker exec "$M" sh -c 'tr "\0" "\n" </proc/$(cat /var/run/docker.pid)/environ | sed -n "s/^PATH=//p"')"
  expect "(a) DOCKER-USER is there, FORWARD jumps to it" yes \
    "$(docker exec "$M" sh -c 'iptables -S DOCKER-USER >/dev/null 2>&1 && iptables -C FORWARD -j DOCKER-USER 2>/dev/null && echo yes || echo no')"
  expect "(b) before the rules: the peer and the outsider connect" "open open" "$(reach peer "$SUB.10") $(reach outsider "$SUB.10")"
  docker exec "$M" iptables -I DOCKER-USER -s 192.0.2.99/32 -j DROP
  rules "$PEER" || ko "(b) the rules could not be written"
  expect "(b) with the rules: the peer connects, the outsider does not" "open refused" "$(reach peer "$SUB.10") $(reach outsider "$SUB.10")"
  expect "(b) … the containers on the server's own network still talk on 9010" open "$(inside)"
  expect "(b) … the rule written by someone else kept" 1 "$(foreign)"
  before=$(ours)
  rules "$PEER"
  expect "(c) a second run: the same rules, nothing added" "$before" "$(ours)"
  rules "$OUTSIDER"
  expect "(d) the peers changed: the stale one refused, the new one let in" "refused open" "$(reach peer "$SUB.10") $(reach outsider "$SUB.10")"
  expect "(d) … one RETURN, the new address's" "1 1" \
    "$(ours | grep -c RETURN) $(ours | grep -c "$OUTSIDER/32 -j RETURN")"
  rules "$PEER"
  dockerd_stop
  dockerd_start || ko "(e) dockerd did not start again"
  sleep 3
  expect "(e) Docker restarted: the rules kept, the outsider still refused" "1 open refused" \
    "$(ours | tail -1) $(reach peer "$SUB.10") $(reach outsider "$SUB.10")"
  expect "(e) … the rule written by someone else kept" 1 "$(foreign)"
  docker restart "$M" >/dev/null
  dockerd_start || ko "(f) dockerd did not start after the reboot"
  sleep 3
  expect "(f) a reboot: nothing kept, the outsider connects" "open" "$(reach outsider "$SUB.10")"
  # Rules written before Docker starts, as a unit ordered Before=docker.service.
  dockerd_stop
  docker restart "$M" >/dev/null
  docker exec "$M" sh -c 'iptables -N DOCKER-USER 2>/dev/null; iptables -C FORWARD -j DOCKER-USER 2>/dev/null || iptables -I FORWARD -j DOCKER-USER'
  rules "$PEER" || ko "(f) the rules could not be written before Docker"
  dockerd_start || ko "(f) dockerd did not start after the rules"
  sleep 3
  expect "(f) written before Docker starts: kept, the peer alone connects" "1 open refused" \
    "$(ours | tail -1) $(reach peer "$SUB.10") $(reach outsider "$SUB.10")"
  expect "(f) … FORWARD jumps to DOCKER-USER once" 1 "$(docker exec "$M" sh -c 'iptables -S FORWARD | grep -c -- "-j DOCKER-USER"')"
  expect "(f) … the containers on the server's own network still talk on 9010" open "$(inside)"
  docker exec -d "$M" sh -c 'nc -lk -p 9011 -e true'
  sleep 1
  expect "(g) IPv6: the server's IPv6 address answers on another port (the control)" open \
    "$(docker exec "$P-outsider" sh -c "nc -z -w 3 ${SUB6}10 9011 && echo open || echo refused")"
  expect "(g) IPv6: a port published on 0.0.0.0 does not answer on the server's IPv6 address" refused \
    "$(reach outsider "${SUB6}10")"
  note "(g) ip6tables DOCKER-USER: $(docker exec "$M" sh -c 'ip6tables -S DOCKER-USER >/dev/null 2>&1 && echo there || echo absent')"
  if [ "${v%%.*}" -ge 29 ]; then
    dockerd_stop
    docker restart "$M" >/dev/null
    # The dind image has no nft, which that backend runs.
    docker exec "$M" apk add -q --no-cache nftables >/dev/null 2>&1 || { ko "(h) no nft for the nftables backend"; continue; }
    dockerd_start --firewall-backend=nftables || { ko "(h) dockerd with nftables did not start"; continue; }
    publish
    note "(h) nftables backend: nft tables $(docker exec "$M" sh -c 'nft list tables 2>/dev/null | tr "\n" " "')"
    expect "(h) nftables backend: DOCKER-USER is not there" no \
      "$(docker exec "$M" sh -c 'iptables -S DOCKER-USER >/dev/null 2>&1 && iptables -C FORWARD -j DOCKER-USER 2>/dev/null && echo yes || echo no')"
  fi
done

echo
if [ "$FAILED" = 0 ]; then echo "ALL OK"; else
  echo "SOME CHECKS FAILED"
  exit 1
fi
