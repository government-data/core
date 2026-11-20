#!/bin/bash
# Shared by grafana_alerting.sh (V2) and grafana_webhook_mtls.sh (V3) of
# decision 0072: Grafana's API as its admin, and a pusher of synthetic
# series (Alloy's textfile collector). Sourced after machines_lib.sh. The
# machines are synthetic; nothing here is a secret but the throwaway
# password, never printed. The files the 2026-11-16 runs provisioned
# (alerting_files) and their webhook sink are in git (afae3e01).
GF_PW=$(openssl rand -hex 16)
MAILPIT_IMAGE=axllent/mailpit:v1.20.4

gf() { # PATH [CURL OPTION...]: Grafana's API as its admin
  local path=$1
  shift
  docker run --rm --network "$NET" -v "$D/gf_admin:/pw:ro" --entrypoint sh "$CURL" -c \
    'p=$1; shift; exec curl -s -m 20 -u "admin:$(cat /pw)" "$@" "http://grafana:3000$p"' sh "$path" "$@"
}

# pusher: Alloy reading $D/prom/*.prom every 5 s and pushing to Mimir as is
# (the vm label in the file). put NAME TEXT writes one file atomically.
pusher_up() {
  mkdir -p "$D/prom" && chmod 755 "$D/prom"
  cat >"$D/pusher.alloy" <<'ALLOY'
prometheus.exporter.unix "files" {
  set_collectors = ["textfile"]
  textfile { directory = "/prom" }
}
prometheus.scrape "files" {
  targets         = prometheus.exporter.unix.files.targets
  scrape_interval = "5s"
  scrape_timeout  = "4s"
  forward_to      = [prometheus.relabel.files.receiver]
}
prometheus.relabel "files" {
  rule {
    source_labels = ["__name__"]
    regex         = "node_.*|qd_.*"
    action        = "keep"
  }
  rule {
    regex  = "__name__|vm|role|mountpoint"
    action = "labelkeep"
  }
  forward_to = [prometheus.remote_write.mimir.receiver]
}
prometheus.remote_write "mimir" {
  endpoint { url = "http://mimir:9009/api/v1/push" }
}
ALLOY
  chmod 644 "$D/pusher.alloy"
  docker run -d --name "$P-pusher" --network "$NET" -v "$D/prom:/prom:ro" -v "$D/pusher.alloy:/etc/p.alloy:ro" \
    "$ALLOY_IMAGE" run --storage.path=/tmp/a --disable-reporting /etc/p.alloy >/dev/null
}
put() { printf '%s\n' "$2" >"$D/prom/.$1" && chmod 644 "$D/prom/.$1" && mv "$D/prom/.$1" "$D/prom/$1.prom"; }
