# shellcheck shell=bash
# Sourced by metrics_app_cert.sh (decision 0071 step 2), the stack up: what
# reaches Mimir, what the limits refuse, and the logs in Loki. Uses P, NET,
# D, T and the helpers from there.

# The lists of point 8 and 10, per job, as the decision gives them: every
# series of a job must have a name and label names in them. Every job also
# keeps __name__, job and instance, and the remote write adds vm and role.
BASE="__name__ job instance vm role"
P3="up process_cpu_seconds_total process_resident_memory_bytes"
H() { echo "$1_bucket $1_sum $1_count"; }
declare -A NAMES LABELS
NAMES[traefik]="$P3 traefik_service_requests_total $(H traefik_service_request_duration_seconds) traefik_entrypoint_requests_total $(H traefik_entrypoint_request_duration_seconds) traefik_tls_certs_not_after"
LABELS[traefik]="code method protocol service entrypoint le cn sans"
NAMES[grafana]="$P3 $(H grafana_http_request_duration_seconds)"
LABELS[grafana]="handler method status_code le"
NAMES[dolt]="$P3 dss_concurrent_connections dss_concurrent_queries dss_connects dss_disconnects $(H dss_query_duration)"
LABELS[dolt]="le"
NAMES[loki]="$P3 loki_distributor_lines_received_total loki_ingester_chunks_flushed_total $(H loki_request_duration_seconds)"
LABELS[loki]="route method status_code ws reason le"
NAMES[mimir]="$P3 cortex_distributor_received_samples_total cortex_ingester_shipper_uploads_total cortex_discarded_samples_total"
LABELS[mimir]="reason group"
NAMES[alloy]="$P3 loki_write_sent_entries_total loki_write_dropped_entries_total prometheus_remote_storage_samples_failed_total"
LABELS[alloy]="component_id host reason remote_name url"
NAMES[postgres]="up pg_up pg_stat_database_numbackends pg_stat_database_xact_commit pg_stat_database_xact_rollback pg_stat_database_deadlocks pg_stat_activity_count"
LABELS[postgres]="datname state"
# The API's metrics file (step 3): this script writes none, so only `up`
# and the collector's own scrape error arrive.
NAMES[files]="up node_textfile_mtime_seconds node_textfile_scrape_error process_cpu_seconds_total process_resident_memory_bytes qd_http_requests_total $(H qd_http_request_duration_seconds) qd_queue_ready_jobs qd_queue_oldest_ready_seconds"
LABELS[files]="file service route status_class kind le"
JOBS="traefik grafana dolt loki mimir alloy postgres files"

mimir() { # API PATH, then curl options: Mimir's answer, as the machine
  curl -sS -G --max-time 15 --cacert "$D/ca.crt" --cert "$D/app.pem" --resolve "qd-app:19009:127.0.0.1" \
    "https://qd-app:19009/prometheus/api/v1/$1" "${@:2}" 2>/dev/null
}
# The jobs whose `up` is VALUE for VM, sorted, one line.
ups() {
  mimir query --data-urlencode "query=up{vm=\"$1\"} == $2" | python3 -I -c '
import json, sys
print(" ".join(sorted(r["metric"]["job"] for r in json.load(sys.stdin)["data"]["result"])))' 2>/dev/null
}
# The series matching SELECTOR: "<count> <metric names> | <label names>".
series() {
  mimir series --data-urlencode "match[]=$1" --data-urlencode "start=$(($(date +%s) - 900))" | python3 -I -c '
import json, sys
d = json.load(sys.stdin)["data"]
print(len(d), " ".join(sorted({s["__name__"] for s in d})), "|", " ".join(sorted({k for s in d for k in s})))' 2>/dev/null
}
outside() { # ALLOWED ACTUAL: the words of ACTUAL not in ALLOWED, or "none"
  python3 -I -c 'import sys; a = set(sys.argv[1].split()); r = [w for w in sys.argv[2].split() if w not in a]; print(" ".join(r) or "none")' "$1" "$2"
}

echo "== (d) the series in Mimir"
want=$(echo $JOBS | tr ' ' '\n' | sort | tr '\n' ' ' | sed 's/ $//')
for _ in $(seq 40); do [ "$(ups verify 1)" = "$want" ] && break; sleep 5; done
expect "up == 1 for each target" "$want" "$(ups verify 1)"
[ "$(ups verify 1)" = "$want" ] || note "up == 0: $(ups verify 0); Alloy's last errors: $(docker logs "$P-alloy-1" 2>&1 | grep -i 'level=error\|level=warn' | tail -3 | cut -c1-300)"
sleep 35 # a second scrape of each, so the histograms and counters are all there
for job in $JOBS; do
  r=$(series "{vm=\"verify\",job=\"$job\"}")
  n=${r%% *} rest=${r#* }
  names=${rest%% |*} labels=${rest#*| }
  note "$job: $n series; names: $names; labels: $labels"
  expect "$job: series arrive, every name in its list" "yes none" \
    "$([ "${n:-0}" -gt 1 ] && echo yes || echo no) $(outside "${NAMES[$job]}" "$names")"
  expect "$job: every label name in its list" none "$(outside "$BASE ${LABELS[$job]}" "$labels")"
done
expect "no series of this VM carries a person's, a client's, a tenant's or a statement's label" "0" \
  "$(series '{vm="verify",__name__=~".+"}' | python3 -I -c '
import sys
bad = {"application_name", "usename", "rolname", "tenant", "user", "query", "queryid", "server", "datid", "serial", "bucket"}
print(len(bad & set(sys.stdin.read().split("|")[1].split())))')"
expect "pg_up is 1: the exporter signed in with the machine's certificate" 1 \
  "$(mimir query --data-urlencode 'query=pg_up{vm="verify"}' | python3 -I -c 'import json, sys; r = json.load(sys.stdin)["data"]["result"]; print(r[0]["value"][1] if r else "none")')"

echo "== (e) the keep lists and the limits, against a fake target"
# Its scrape errors are logged at debug: a file of the test's own says so.
echo 'logging { level = "debug" }' >"$D/www/debug.alloy" && chmod 644 "$D/www/debug.alloy"
docker run -d --name "$P-alloy-fake" --network "${P}_default" --restart no --add-host "qd-app:$NET.10" \
  -e OBS_VM=verify-fake -e METRICS_ROUTER_ADDRESS=fake:8000 -e LOKI_METRICS_ADDRESS=fake:8000 \
  -e MIMIR_METRICS_ADDRESS=fake:8000 -e METRICS_PUSH_URL=https://qd-app:9009/api/v1/push \
  -e METRICS_CA_FILE=/run/qd-node/ca.crt -e METRICS_CERT_FILE=/run/qd-node/machine.pem \
  -e GRAFANA_METRICS_PASSWORD_FILE=/run/s/grafana_metrics_password \
  -v "$D/agent:/run/qd-node:ro" -v "$D/secrets:/run/s:ro" -v "$T/infra/observability/alloy/app.alloy:/etc/qd-alloy/app.alloy:ro" \
  -v "$D/www/debug.alloy:/etc/qd-alloy/debug.alloy:ro" \
  "$(python3 -I -c 'import json, sys; print(json.load(open(sys.argv[1]))["services"]["alloy"]["image"])' "$D/cert.json")" \
  run --storage.path=/tmp/alloy --disable-reporting /etc/qd-alloy >/dev/null
for _ in $(seq 30); do [ -n "$(ups verify-fake 1)" ] && break; sleep 5; done
sleep 35
expect "the fake Grafana answers, the fake Traefik (6,000 samples) and Dolt (25 labels) fail whole" \
  "alloy files grafana|dolt loki mimir traefik" "$(ups verify-fake 1)|$(ups verify-fake 0)"
expect "Alloy says why: the sample limit and the label limit" "yes yes" \
  "$(docker logs "$P-alloy-fake" 2>&1 | grep -qi 'sample.limit' && echo yes || echo no) $(docker logs "$P-alloy-fake" 2>&1 | grep -qi 'label.limit' && echo yes || echo no)"
expect "the allowed metric arrives without usename, application_name or tenant" \
  "2 grafana_http_request_duration_seconds_count grafana_http_request_duration_seconds_sum | __name__ handler instance job method role status_code vm" \
  "$(series '{vm="verify-fake",__name__=~"grafana_http_request_duration_seconds_(count|sum)"}')"
expect "the metric outside the lists, and nothing of the failed scrapes, arrives" 0 \
  "$(series '{vm="verify-fake",__name__=~"grafana_verify_unlisted_total|traefik_service_requests_total|dss_connects"}' | cut -d' ' -f1)"

echo "== (f) the logs"
lines=$(curl -sS -G --max-time 15 --cacert "$D/ca.crt" --cert "$D/app.pem" --resolve "qd-app:13100:127.0.0.1" \
  "https://qd-app:13100/loki/api/v1/query_range" --data-urlencode 'query={vm="verify",compose_project=~"qd-verify-metrics-app.*"}' \
  --data-urlencode limit=50 --data-urlencode "start=$(($(date +%s) - 900))000000000" 2>/dev/null | python3 -I -c '
import json, sys
r = json.load(sys.stdin)["data"]["result"]
print(len(r), sum(len(s["values"]) for s in r))' 2>/dev/null)
expect "the containers' lines still reach Loki beside the metrics (streams, lines)" yes \
  "$([ "${lines%% *}" -gt 0 ] 2>/dev/null && echo yes || echo "no: ${lines:-none}")"
note "Loki: ${lines:-none} (streams, lines); memory: Alloy $(docker stats --no-stream --format '{{.MemUsage}}' "$P-alloy-1" | cut -d/ -f1), Mimir $(docker stats --no-stream --format '{{.MemUsage}}' "$P-mimir-1" | cut -d/ -f1)"
