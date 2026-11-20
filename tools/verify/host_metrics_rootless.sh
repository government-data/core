#!/bin/bash
# Verification V1 of decision 0072, brought to step 1's files (point 2 as
# amended on 2026-11-16): the host's metrics from Alloy v1.21.3's
# prometheus.exporter.unix, run by the repository's own compose files with
# the repository's own configurations, pushed to a throwaway Mimir.
#
# A worker's Alloy: infra/worker/docker-compose.yml's `alloy` service with
# infra/observability/alloy/worker.alloy beside it, as join.sh writes them
# (65532, read_only, no capability, no-new-privileges, its server on its
# loopback), its watched path Docker's data (this machine's /var/lib/docker,
# the compose file's default, bound read-only and not recursively) and the
# host's /proc/1/net at /host/proc1/net; an override adds only the network,
# no TLS (the files' empty CA, as in development), two more watched paths to
# compare with statfs (a host folder, `spill`, and a 64 MiB tmpfs,
# `drive1`), and a scrape with a sample_limit of 50 to see it fail whole.
# The application VM's Alloy: infra/observability/docker-compose.yml's
# `alloy` service with config.alloy and app.alloy (root, the Docker socket
# for 0026's logs, here of no project), its watched paths PostgreSQL's,
# Dolt's and Loki's volumes (throwaway ones, under this run's names).
#
# Compared with this host's /proc and statfs read directly at the same
# moment: CPU, memory, load, disks, boot time, each watched filesystem and
# the host's interfaces. Neither container has /host/proc, and neither
# reaches a synthetic "victim" container of the worker's user (a marker
# variable and a file; only counts are printed): the whole-/proc probes of
# the first run (2026-11-16) are kept as that check. The lake roles are
# step 2's (join_lake_pool.sh, join_lake_replica.sh). On WSL2 the disks,
# the clock and the interfaces are the utility VM's, not a server's.
#
# Containers, volumes and projects qd-verify-host-…, network
# qd-verify-host-net, removed on exit; no port published; nothing of the dev
# stack touched (no dev volume is mounted, the repository's .env is not
# read). Run it alone, from any directory:
#
#   bash tools/verify/host_metrics_rootless.sh
P=qd-verify-host
. "$(dirname "$0")/machines_lib.sh"
EXCL='^(loop.*|ram.*|veth.*|docker.*|br-.*|lo)$'
DOCKER_ROOT=/var/lib/docker
# The observability compose needs these set to be read; none is used here.
OBS_ENV=(LOKI_S3_ACCESS_KEY=unused LOKI_S3_SECRET_KEY=unused MIMIR_S3_ACCESS_KEY=unused MIMIR_S3_SECRET_KEY=unused
  QD_SECRETS_DIR="$D/secrets" QD_METRICS_VOLUME="$P-app-metrics" APP_STACK="$P-app" OBS_VM=verify-app
  OBS_LOG_PROJECTS=qd-verify-no-project METRICS_PUSH_URL=http://mimir:9009/api/v1/push)
dc_worker() { OBS_VM=verify-worker METRICS_PUSH_URL=http://mimir:9009/api/v1/push docker compose -p "$P-worker" \
  --env-file "$D/empty.env" -f "$D/worker/compose.yml" -f "$D/worker/verify.yml" --profile metrics "$@"; }
dc_app() { env "${OBS_ENV[@]}" docker compose -p "$P-app" --project-directory "$ROOT" --env-file "$D/empty.env" \
  -f "$ROOT/infra/observability/docker-compose.yml" -f "$D/app-verify.yml" "$@"; }
start_run
trap 'kill $WATCHDOG 2>/dev/null; dc_worker down -v >/dev/null 2>&1; dc_app down -v >/dev/null 2>&1; cleanup' EXIT

# --- The role's files, as join.sh writes them, and this run's overrides ---------
mkdir -p "$D/worker" "$D/spill" "$D/secrets"
: >"$D/empty.env"
cp "$ROOT/infra/worker/docker-compose.yml" "$D/worker/compose.yml"
cp "$ROOT/infra/observability/alloy/worker.alloy" "$D/worker/worker.alloy"
for s in grafana_admin_password grafana_oidc_client_secret grafana_pg_password grafana_metrics_password; do
  printf 'unused' >"$D/secrets/$s"
done
cat >"$D/worker/limit.alloy" <<'ALLOY'
// 0071 point 11's limit, set too low on purpose: the whole scrape fails, up 0.
prometheus.scrape "limit" {
  targets         = prometheus.exporter.unix.host.targets
  scrape_interval = "15s"
  sample_limit    = 50
  forward_to      = [prometheus.relabel.limit.receiver]
}
prometheus.relabel "limit" {
  rule {
    source_labels = ["__name__"]
    regex         = "up"
    action        = "keep"
  }
  rule {
    target_label = "job"
    replacement  = "limit"
  }
  rule {
    regex  = "__name__|job"
    action = "labelkeep"
  }
  forward_to = [prometheus.remote_write.mimir.receiver]
}
ALLOY
cat >"$D/worker/verify.yml" <<YML
networks:
  default: { name: $NET, external: true }
services:
  alloy:
    environment: { METRICS_CA_FILE: "", METRICS_CERT_FILE: "" }
    volumes:
      - ./limit.alloy:/etc/qd-alloy/limit.alloy:ro
      - $D/spill:/host/watch/spill:ro
      - { type: tmpfs, target: /host/watch/drive1, tmpfs: { size: 67108864 } }
YML
printf 'networks:\n  default: { name: %s, external: true }\n' "$NET" >"$D/app-verify.yml"
chmod 755 "$D/worker" "$D/spill" "$D/secrets"
chmod 644 "$D/worker"/* "$D/app-verify.yml" "$D/secrets"/* "$D/empty.env"

victim() { # NAME UID: a synthetic process with a marker variable and a file
  docker run -d --name "$P-$1" --user "$2:$2" -e QD_VERIFY_MARKER=synthetic --tmpfs "/work:uid=$2,gid=$2,mode=0700" \
    alpine:3.20 sh -c 'echo synthetic >/work/f; exec sleep 900' >/dev/null
}
raw() { # CONTAINER COMPONENT: the exporter's output, read on Alloy's loopback
  docker run --rm --network "container:$1" "$CURL" -s -m 20 "http://127.0.0.1:12345/api/v0/component/prometheus.exporter.unix.$2/metrics"
}
m() { # FILE NAME [LABEL=VALUE]: a value from an exposition file
  python3 -I -c '
import re, sys
f, name = sys.argv[1], sys.argv[2]
want = sys.argv[3] if len(sys.argv) > 3 else None
for line in open(f):
    mt = re.match(r"([a-zA-Z_:][\w:]*)(\{.*\})? (\S+)$", line.strip())
    if mt and mt.group(1) == name and (want is None or want.replace("=", "=\"") + "\"" in (mt.group(2) or "")):
        print(int(float(mt.group(3))) if float(mt.group(3)).is_integer() else mt.group(3)); break
else:
    print("none")' "$@"
}
labels() { # FILE NAME LABEL: the label's values, sorted, space-separated
  grep "^$2{" "$1" | grep -o "$3=\"[^\"]*\"" | cut -d'"' -f2 | sort -u | tr '\n' ' ' | sed 's/ $//'
}
values() { # LABEL SELECTOR: a label's values in Mimir, sorted, space-separated
  mimir_get "/prometheus/api/v1/label/$1/values" -G --data-urlencode "match[]=$2" |
    python3 -I -c 'import json, sys; print(" ".join(sorted(json.load(sys.stdin)["data"])))'
}
host_ifaces() { awk -F: 'NR > 2 { gsub(/ /, "", $1); print $1 }' /proc/net/dev | grep -Ev "$EXCL" | sort | tr '\n' ' ' | sed 's/ $//'; }
host_rx() { awk -F'[: ]+' -v d="$1" '{ sub(/^ +/, "") } $1 == d { print $2 }' /proc/net/dev; }
fs_size() { stat -f -c '%b %S' "$1" | awk '{ printf "%d\n", $1 * $2 }'; }
within() { awk -v a="$1" -v b="$2" 'BEGIN { d = (a - b) / b; print (d < 0.01 && d > -0.01) ? "yes" : "no (" a " " b ")" }'; }
# probe CONTAINER VICTIM: whether /host/proc exists, any /proc entry of the
# victim's host pid, how many of its processes' environments hold the
# marker, and the victim's file read through /proc/<pid>/root.
probe() {
  docker exec "$1" sh -c 'v=$1; [ -e /host/proc ] && echo present || echo absent
for d in /proc /host/proc /host/proc1; do [ -e "$d/$v" ] && echo "$d/$v"; done
grep -l "^QD_VERIFY_MARKER=synthetic" /proc/[0-9]*/environ 2>/dev/null | wc -l
cat "/proc/$v/root/work/f" 2>/dev/null || echo -' sh "$(docker inspect -f '{{.State.Pid}}' "$P-$2")" | paste -sd' '
}

mimir_up || exit 1
victim same 65532
dc_worker up -d --no-build alloy >"$D/up.log" 2>&1 || { ko "the worker's compose did not start Alloy: $(tail -2 "$D/up.log")"; exit 1; }
dc_app up -d --no-build --no-deps alloy >"$D/up.log" 2>&1 || { ko "the observability compose did not start Alloy: $(tail -2 "$D/up.log")"; exit 1; }
W=$P-worker-alloy-1 A=$P-app-alloy-1
sleep 10

echo "== the worker's Alloy, from its compose file"
expect "it runs as 65532, its root filesystem read-only, no capability in its bounding set, no new privileges" "65532 true 0000000000000000 1" \
  "$(docker top "$W" -eo pid,uid | awk 'NR == 2 { print $2 }') $(docker inspect -f '{{.HostConfig.ReadonlyRootfs}}' "$W") $(docker exec "$W" sh -c 'grep CapBnd /proc/self/status | cut -f2; grep NoNewPrivs /proc/self/status | cut -f2' | paste -sd' ')"
expect "its server on its loopback alone: refused from another container on its network" 7 \
  "$(docker run --rm --network "$NET" "$CURL" -s -o /dev/null -m 5 "http://$W:12345/" >/dev/null 2>&1; echo $?)"
before_rx=$(host_rx eth0)
raw "$W" host >"$D/host.txt"
raw "$W" network >"$D/net.txt"
after_rx=$(host_rx eth0)
for c in cpu meminfo loadavg filesystem diskstats timex stat; do
  expect "collector $c succeeds from the container's own /proc" 1 "$(m "$D/host.txt" node_scrape_collector_success "collector=$c")"
done
expect "the host exporter runs no netdev (it would read the container's namespace)" none "$(m "$D/host.txt" node_scrape_collector_success collector=netdev)"
expect "MemTotal: the host's" "$(awk '/^MemTotal:/ { print $2 * 1024 }' /proc/meminfo)" "$(m "$D/host.txt" node_memory_MemTotal_bytes)"
expect "CPUs: the host's" "$(grep -c '^cpu[0-9]' /proc/stat)" "$(labels "$D/host.txt" node_cpu_seconds_total cpu | wc -w)"
expect "boot time: the host's btime" "$(awk '/^btime/ { print $2 }' /proc/stat)" "$(m "$D/host.txt" node_boot_time_seconds)"
expect "load1 within 1 of the host's" yes "$(awk -v a="$(m "$D/host.txt" node_load1)" -v b="$(cut -d' ' -f1 /proc/loadavg)" 'BEGIN { d = a - b; print (d < 1 && d > -1) ? "yes" : "no" }')"
expect "disks: the host's whole devices" \
  "$(awk '$3 !~ /^(z?ram|loop|fd|(h|s|v|xv)d[a-z]|nvme[0-9]+n[0-9]+p)[0-9]+$/ { print $3 }' /proc/diskstats | sort | tr '\n' ' ' | sed 's/ $//')" \
  "$(labels "$D/host.txt" node_disk_read_bytes_total device)"
note "timex: sync_status $(m "$D/host.txt" node_timex_sync_status), offset $(m "$D/host.txt" node_timex_offset_seconds) s; raw samples a scrape: $(grep -cv '^#' "$D/host.txt") and $(grep -cv '^#' "$D/net.txt") (sample_limit 1,000)"

echo "== the worker's watched paths"
expect "docker: the size of the filesystem under Docker's data" "$(fs_size "$DOCKER_ROOT")" "$(m "$D/host.txt" node_filesystem_size_bytes mountpoint=/host/watch/docker)"
expect "docker: its free space within 1 % of statfs" yes \
  "$(within "$(m "$D/host.txt" node_filesystem_avail_bytes mountpoint=/host/watch/docker)" "$(stat -f -c '%a %S' "$DOCKER_ROOT" | awk '{ printf "%d\n", $1 * $2 }')")"
note "$(awk '$5 ~ "^/var/lib/docker/"' /proc/self/mountinfo | wc -l) mounts under $DOCKER_ROOT on this host"
expect "docker bound not recursively: no mount under it listed" 0 "$(grep -c 'mountpoint="/host/watch/docker/' "$D/host.txt")"
expect "spill (a host folder): the size of the filesystem underneath" "$(fs_size "$D/spill")" "$(m "$D/host.txt" node_filesystem_size_bytes mountpoint=/host/watch/spill)"
expect "drive1 (a 64 MiB filesystem of its own): its size exactly" 67108864 "$(m "$D/host.txt" node_filesystem_size_bytes mountpoint=/host/watch/drive1)"

echo "== the worker's network: only /proc/1/net, never the whole /proc"
expect "the network exporter: the host's interfaces, as /proc/net/dev lists them less the excluded" "$(host_ifaces)" "$(labels "$D/net.txt" node_network_receive_bytes_total device)"
rx=$(m "$D/net.txt" node_network_receive_bytes_total device=eth0)
expect "the host's eth0 bytes received, between two direct reads of /proc/net/dev" yes \
  "$([ "$before_rx" -le "$rx" ] && [ "$rx" -le "$after_rx" ] && echo yes || echo "no ($before_rx $rx $after_rx)")"
expect "/host/proc1 holds net alone, no process" net "$(docker exec "$W" ls /host/proc1)"
expect "the worker's Alloy: no /host/proc, nothing of a same-user container's (environment, files)" "absent 0 -" "$(probe "$W" same)"

echo "== the application VM's Alloy, from the observability compose"
note "it runs as uid $(docker top "$A" -eo pid,uid | awk 'NR == 2 { print $2 }') with the Docker socket, as its compose file says (step 1 does not change that)"
raw "$A" host >"$D/app-host.txt"
raw "$A" network >"$D/app-net.txt"
for p in postgres dolt loki; do
  expect "$p's volume: the size of the filesystem under Docker's data" "$(fs_size "$DOCKER_ROOT")" "$(m "$D/app-host.txt" node_filesystem_size_bytes "mountpoint=/host/watch/$p")"
done
expect "its network exporter: the host's interfaces" "$(host_ifaces)" "$(labels "$D/app-net.txt" node_network_receive_bytes_total device)"
expect "the application VM's Alloy: no /host/proc, nothing of another container's" "absent 0 -" "$(probe "$A" same)"

echo "== pushed and kept"
for _ in $(seq 40); do
  [ "$(scalar 'count(node_filesystem_avail_bytes{vm="verify-worker"})')" = 3 ] &&
    [ "$(scalar 'count(node_filesystem_avail_bytes{vm="verify-app"})')" = 3 ] && break
  sleep 5
done
sleep 35
expect "Mimir: node_filesystem_avail_bytes of each watched path, the worker" "docker drive1 spill" "$(values mountpoint 'node_filesystem_avail_bytes{vm="verify-worker"}')"
expect "Mimir: node_filesystem_avail_bytes of each watched path, the application VM" "dolt loki postgres" "$(values mountpoint 'node_filesystem_avail_bytes{vm="verify-app"}')"
for vm in verify-worker verify-app; do
  expect "Mimir: $vm's host interfaces" "$(host_ifaces)" "$(values device "node_network_receive_bytes_total{vm=\"$vm\"}")"
  expect "Mimir: $vm's interfaces only from the network exporter" network "$(values instance "node_network_receive_bytes_total{vm=\"$vm\"}")"
  expect "Mimir: $vm's two host scrapes up (sample_limit 1,000, label_limit 20)" 2 "$(scalar "sum(up{vm=\"$vm\",job=\"host\"})")"
  total=$(scalar "count({vm=\"$vm\",job=\"host\"})")
  by=$(promql "count by (__name__) ({vm=\"$vm\",job=\"host\"})" | python3 -I -c '
import json, sys, collections
c = collections.Counter()
for r in json.load(sys.stdin)["data"]["result"]:
    n = r["metric"]["__name__"]
    k = "cpu" if n.startswith("node_cpu") or n.startswith("node_load") else "memory" if n.startswith("node_memory") else \
        "filesystems" if n.startswith("node_filesystem") else "disks" if n.startswith("node_disk") else \
        "network" if n.startswith("node_network") else "up" if n == "up" else "clock+boot"
    c[k] += int(r["value"][1])
print(" ".join(f"{k} {v}" for k, v in sorted(c.items())))')
  note "$vm: $total series after the keep list ($by)"
  expect "$vm: under 300 series (0072 point 3; above it the budget goes to 30,000)" yes "$([ "$total" != none ] && [ "$total" -le 300 ] && echo yes || echo no)"
done
expect "label names kept: 0072's and the push's" "__name__ cpu device instance job mode mountpoint role vm" \
  "$(mimir_get /prometheus/api/v1/labels -G --data-urlencode 'match[]={job="host"}' | python3 -I -c 'import json, sys; print(" ".join(sorted(json.load(sys.stdin)["data"])))')"
expect "a scrape past its sample_limit (50) fails whole: up 0" 0 "$(scalar 'up{vm="verify-worker",job="limit"}')"
note "Alloy $(docker run --rm "$ALLOY_IMAGE" --version 2>/dev/null | head -1 | cut -d' ' -f3), Mimir 3.2.4, Docker $(docker version -f '{{.Server.Version}}'), Compose $(docker compose version --short), kernel $(uname -r)"
echo "$PASSED passed, $FAILED failed"
[ $FAILED = 0 ] && echo "ALL OK" || echo "SOME FAILED"
exit $((FAILED > 0))
