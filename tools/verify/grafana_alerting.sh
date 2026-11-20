#!/bin/bash
# Verification V2 of decision 0072, brought to step 4's generated files:
# Grafana 13.2.5 provisioned with the repository's
# infra/observability/grafana/provisioning/alerting/*.yml and
# datasources/mimir.yml, unchanged (tools/grafana/alerts.mjs writes them),
# against a throwaway Mimir 3.2.4 fed synthetic series (machine verify-a,
# role lake, by Alloy's textfile collector) and a throwaway Mailpit v1.20.4
# (STARTTLS with a throwaway CA, SMTP authentication required). Grafana gets
# the observability compose's environment for its mail (GF_SMTP_*,
# GRAFANA_ALERT_EMAILS, the CA as SSL_CERT_FILE, both content types), with
# synthetic mailboxes and throwaway secrets, never printed.
#
# It shows: the four files provisioned (provenance file); point 6's rules as
# amended (severity on all but the heartbeat, noDataState OK on the others,
# execErrState Error); `$VAR` interpolated in the contact points; the policy
# tree of point 7; the SMTP password read from its file; one mail, in both
# languages (text and HTML parts), for a filled disk, nothing for the
# heartbeat, and one for its resolution; with Mimir stopped, a mail that
# says "no machine" with the rule's name. The intervals are the files'
# (rules every minute, disk_low for 5 m, group_wait 30 s, group_interval
# 5 m), so it runs some 20 minutes. The 2026-11-16 run's other checks
# (restart, NoData, repeats; files written by grafana_alerting_lib.sh) are
# in its record and in git (82c08e58).
#
# Containers qd-verify-alerting-…, network and volumes of that name, removed
# on exit; no port published. Run it alone.
#
#   bash tools/verify/grafana_alerting.sh
P=qd-verify-alerting
. "$(dirname "$0")/machines_lib.sh"
. "$(dirname "$0")/grafana_alerting_lib.sh"
PROV=$ROOT/infra/observability/grafana/provisioning
EMAILS="ops@example.org;infra@example.com"
start_run
ca ca
cert mailpit ca "/O=qd/CN=mailpit" mailpit
openssl rand -hex 12 | tr -d '\n' >"$D/smtp_pw" && chmod 644 "$D/smtp_pw"
fs() { # AVAIL: verify-a's drive1, of 1 TB
  put fs "node_filesystem_avail_bytes{vm=\"verify-a\",role=\"lake\",mountpoint=\"drive1\"} $1
node_filesystem_size_bytes{vm=\"verify-a\",role=\"lake\",mountpoint=\"drive1\"} 1000000000000"
}
mail() { docker run --rm --network "$NET" "$CURL" -s -m 10 "http://mailpit:8025/api/v1/$1"; }
# mails FILTER: the subjects of the mails matching a Python expression over s (the subject)
mails() {
  mail messages | python3 -I -c '
import json, sys
for m in json.load(sys.stdin)["messages"]:
    s = m["Subject"]
    if eval(sys.argv[1], {"s": s}):
        print(m["ID"], s)' "$1"
}
wait_mail() { # SECONDS FILTER: until a mail matches
  local i
  for i in $(seq $(($1 / 5))); do [ -n "$(mails "$2")" ] && return 0; sleep 5; done
  return 1
}
part() { python3 -I -c 'import json, sys; print(json.load(open(sys.argv[1]))[sys.argv[2]])' "$@"; }
rules_json() { gf /api/v1/provisioning/alert-rules; }

mimir_up || exit 1
pusher_up
fs 500000000000
docker run -d --name "$P-mailpit" --network "$NET" --network-alias mailpit -v "$D:/c:ro" \
  -e MP_SMTP_AUTH="qd-grafana:$(cat "$D/smtp_pw")" -e MP_SMTP_TLS_CERT=/c/mailpit.crt -e MP_SMTP_TLS_KEY=/c/mailpit.key \
  -e MP_SMTP_REQUIRE_STARTTLS=1 "$MAILPIT_IMAGE" >/dev/null
mkdir -p "$D/prov/datasources" "$D/prov/alerting" "$D/prov/dashboards" "$D/prov/plugins" "$D/prov/access-control"
cp "$PROV/datasources/mimir.yml" "$D/prov/datasources/"
cp "$PROV"/alerting/*.yml "$D/prov/alerting/"
chmod -R a+rX "$D/prov"
printf '%s' "$GF_PW" >"$D/gf_admin" && chmod 644 "$D/gf_admin"
docker run -d --name "$P-grafana" --network "$NET" --network-alias grafana \
  -v "$P-grafana-data:/var/lib/grafana" -v "$D/prov:/etc/grafana/provisioning:ro" -v "$D/gf_admin:/run/secrets/gf_admin:ro" \
  -v "$D/smtp_pw:/run/secrets/grafana_smtp_password:ro" -v "$D/ca.crt:/etc/qd/smtp-ca.crt:ro" \
  -e GF_SECURITY_ADMIN_PASSWORD__FILE=/run/secrets/gf_admin -e GF_ANALYTICS_REPORTING_ENABLED=false \
  -e GF_ANALYTICS_CHECK_FOR_UPDATES=false -e GF_NEWS_NEWS_FEED_ENABLED=false -e GF_LOG_LEVEL=warn \
  -e MIMIR_URL=http://mimir:9009 -e MIMIR_SERVER_NAME= -e INTERNAL_DOMAIN=internal.verify.example \
  -e GRAFANA_ALERT_EMAILS="$EMAILS" -e GF_SMTP_ENABLED=true -e GF_SMTP_HOST=mailpit:1025 -e GF_SMTP_USER=qd-grafana \
  -e GF_SMTP_PASSWORD__FILE=/run/secrets/grafana_smtp_password -e GF_SMTP_FROM_ADDRESS=alerts@example.org \
  -e "GF_SMTP_FROM_NAME=Quality Data" -e GF_SMTP_STARTTLS_POLICY=MandatoryStartTLS -e SSL_CERT_FILE=/etc/qd/smtp-ca.crt \
  -e GF_EMAILS_CONTENT_TYPES=text/html,text/plain "$GRAFANA_IMAGE" >/dev/null
for _ in $(seq 90); do [ "$(gf /api/health -o /dev/null -w '%{http_code}')" = 200 ] && break; sleep 2; done
[ "$(gf /api/health -o /dev/null -w '%{http_code}')" = 200 ] || {
  ko "Grafana not ready: $(docker logs "$P-grafana" 2>&1 | grep -i error | tail -3 | cut -c1-300)"
  exit 1
}

echo "== provisioned from the generated files"
expect "rules, contact points, policy and template carry provenance file" "file file file file" \
  "$(gf /api/v1/provisioning/alert-rules/qd-disk-low | python3 -I -c 'import json, sys; print(json.load(sys.stdin).get("provenance"))') \
$(gf /api/v1/provisioning/contact-points | python3 -I -c 'import json, sys; print(json.load(sys.stdin)[0].get("provenance"))') \
$(gf /api/v1/provisioning/policies | python3 -I -c 'import json, sys; print(json.load(sys.stdin).get("provenance"))') \
$(gf /api/v1/provisioning/templates/qd-mail | python3 -I -c 'import json, sys; print(json.load(sys.stdin).get("provenance"))')"
expect "the rules of alerts.json, all provisioned" \
  "$(python3 -I -c 'import json, sys; print(sum(len(g["rules"]) for g in json.load(open(sys.argv[1]))["groups"]))' \
    "$ROOT/infra/observability/grafana/definition/alerts.json")" \
  "$(rules_json | python3 -I -c 'import json, sys; print(len(json.load(sys.stdin)))')"
expect "severity on every rule but the heartbeat; noDataState OK but the heartbeat's; execErrState Error" "yes" \
  "$(rules_json | python3 -I -c '
import json, sys
ok = True
for r in json.load(sys.stdin):
    hb = r["labels"]["qd_alert"] == "alerting_heartbeat"
    ok &= ("severity" in r["labels"]) != hb and (r["noDataState"] == "OK") != hb and r["execErrState"] == "Error"
    ok &= r["labels"].get("severity") in (None, "warning", "critical") and not r["labels"].get("_readonly")
print("yes" if ok else "no")')"
expect "no rule reads node_filesystem_readonly (point 6 as amended)" no \
  "$(rules_json | grep -q readonly && echo yes || echo no)"
expect "\$GRAFANA_ALERT_EMAILS interpolated in the mail contact point" "$EMAILS" \
  "$(gf /api/v1/provisioning/contact-points | python3 -I -c 'import json, sys; print([c for c in json.load(sys.stdin) if c["type"] == "email"][0]["settings"]["addresses"])')"
expect "\${INTERNAL_DOMAIN} interpolated in the heartbeat's webhook" \
  "qd-api-heartbeat https://internal.verify.example:9443/api/internal/machines/alerts" \
  "$(gf /api/v1/provisioning/contact-points | python3 -I -c '
import json, sys
c = [c for c in json.load(sys.stdin) if c["type"] == "webhook"][0]; print(c["name"], c["settings"].get("url"))')"
expect "the policy tree: root to mail, the heartbeat's child route (point 7)" \
  "qd-mail-and-api vm,qd_alert 30s 5m 12h | qd-api-heartbeat qd_alert=alerting_heartbeat 1m 5m" \
  "$(gf /api/v1/provisioning/policies | python3 -I -c '
import json, sys
p = json.load(sys.stdin); r = p["routes"][0]; m = r["object_matchers"][0]
print(p["receiver"], ",".join(p["group_by"]), p["group_wait"], p["group_interval"], p["repeat_interval"], "|",
      r["receiver"], m[0] + m[1] + m[2], r["group_interval"], r["repeat_interval"])')"
expect "Mailpit refuses a wrong SMTP password (so Grafana's delivery proves the file's)" 67 \
  "$(echo x | docker run --rm -i --network "$NET" -v "$D:/c:ro" "$CURL" -s --ssl-reqd --cacert /c/ca.crt --user qd-grafana:wrong \
    --mail-from a@example.org --mail-rcpt b@example.org -T - smtp://mailpit:1025 >/dev/null 2>&1; echo $?)"

echo "== a filled disk: one mail, both languages"
fs 70000000000
FIRING='s.startswith("[ALERTA / ALERT] verify-a: ")'
wait_mail 600 "$FIRING" || ko "no mail for verify-a's disk within 600 s"
sleep 30
mails 'True' >"$D/all1"
expect "one mail by now, for verify-a's disk_low" 1 "$(wc -l <"$D/all1")"
ID=$(mails "$FIRING" | head -1 | cut -d' ' -f1)
mail "message/$ID" >"$D/msg.json"
note "subject: $(part "$D/msg.json" Subject)"
part "$D/msg.json" Text | sed -n '1,12p' | sed 's/^/    text | /'
python3 -I - "$D/msg.json" <<'PY' | sed 's/^/    html | /'
import html, json, re, sys
h = json.load(open(sys.argv[1]))["HTML"]
t = html.unescape(re.sub(r"\s+", " ", re.sub(r"<(style|head)[^>]*>.*?</\1>|<[^>]+>", " ", h, flags=re.S)))
i = t.find("Español")
print(t[max(i - 40, 0):i + 400].strip())
PY
expect "the subject: both languages' summaries, the free space and the limit" yes \
  "$(part "$D/msg.json" Subject | grep -qF '[ALERTA / ALERT] verify-a: Queda poco disco en verify-a (drive1): 7% libre, menos de 10 % / Disk almost full on verify-a (drive1): 7% free, under 10 %' && echo yes || echo no)"
expect "to both mailboxes, from the configured address" "alerts@example.org infra@example.com,ops@example.org" \
  "$(python3 -I -c '
import json, sys
m = json.load(open(sys.argv[1]))
print(m["From"]["Address"], ",".join(sorted(t["Address"] for t in m["To"])))' "$D/msg.json")"
for p in Text HTML; do
  expect "the $p part: Spanish first, then English, each alert with its state" yes \
    "$(python3 -I -c '
import html, json, sys
t = html.unescape(json.load(open(sys.argv[1]))[sys.argv[2]])
i = 0  # each after the one before: the HTML part repeats the subject in its title
for x in ("Español:", "Queda poco disco en verify-a (drive1)", "(activa)", "English:", "Disk almost full on verify-a (drive1)", "(firing)"):
    i = t.find(x, i) if i >= 0 else -1
print("yes" if i >= 0 else "no")' "$D/msg.json" "$p")"
done

echo "== its resolution"
fs 500000000000
RESOLVED='s.startswith("[RESUELTA / RESOLVED] verify-a: ")'
wait_mail 420 "$RESOLVED" || ko "no resolution mail within 420 s"
ID=$(mails "$RESOLVED" | head -1 | cut -d' ' -f1)
mail "message/$ID" >"$D/res.json"
note "subject: $(part "$D/res.json" Subject)"
expect "the resolution: both states, in both parts" "yes yes" \
  "$(for p in Text HTML; do python3 -I -c '
import html, json, sys
t = html.unescape(json.load(open(sys.argv[1]))[sys.argv[2]])
r = t.find("(resuelta)"); e = t.find("English:", r)
print("yes" if 0 <= r < e < t.find("(resolved)", e) else "no")' "$D/res.json" "$p"; done | xargs)"
expect "two mails in all: the alert and its resolution (none for the heartbeat)" 2 "$(mails 'True' | wc -l)"

echo "== Mimir stopped: an alert with no machine"
docker stop "$P-mimir" >/dev/null
NOVM='"sin máquina / no machine" in s'
wait_mail 600 "$NOVM" || ko "no mail without a machine within 600 s"
ID=$(mails "$NOVM" | head -1 | cut -d' ' -f1)
mail "message/$ID" >"$D/err.json"
note "subject: $(part "$D/err.json" Subject)"
part "$D/err.json" Text | sed -n '1,6p' | sed 's/^/    text | /'
expect "it names the rule in both languages, not an empty summary" yes \
  "$(python3 -I -c '
import json, re, sys
t = json.load(open(sys.argv[1]))["Text"]
print("yes" if re.search(r"Métricas no disponibles: Grafana no puede evaluar la regla \w+", t)
      and re.search(r"Metrics not available: Grafana cannot evaluate rule \w+", t) and "[no value]" not in t else "no")' "$D/err.json")"
expect "the heartbeat's DatasourceError is not mailed (its own route)" 0 "$(mails '"alerting_heartbeat" in s' | wc -l)"

echo "   Grafana $(gf /api/health | python3 -I -c 'import json, sys; print(json.load(sys.stdin)["version"])'), Mimir 3.2.4, Alloy v1.21.3, Mailpit v1.20.4"
echo "$PASSED passed, $FAILED failed"
[ $FAILED = 0 ] && echo "ALL OK" || echo "SOME FAILED"
exit $((FAILED > 0))
