#!/bin/sh
# Grafana's own secrets as files (decision 0026; AGENTS.md: secrets are read
# from files), mounted by docker-compose.yml as Docker secrets. Run before
# `task obs:up`, which runs it; running it again changes nothing:
#   infra/observability/setup-secrets.sh
# It writes, in secrets/ (git-ignored; QD_SECRETS_DIR in .env moves it):
#   grafana_admin_password     the emergency admin's: GRAFANA_ADMIN_PASSWORD
#                              of an older .env when it is there (Grafana keeps
#                              the password of its first start in its own
#                              database), otherwise a random one
#   grafana_metrics_password   the basic auth of Grafana's /metrics, which
#                              Alloy sends (decision 0071, point 7), random
#   grafana_oidc_client_secret GRAFANA_OIDC_CLIENT_SECRET of an older .env,
#                              otherwise empty until it is set by hand
#   grafana_smtp_password      its SMTP user's, for the alerts' mail (decision
#                              0072, point 9): GRAFANA_SMTP_PASSWORD from the
#                              environment (the installer passes it, never
#                              in .env), otherwise empty; an empty file is
#                              written again when a value is given
#   grafana_pg_password        its read-only PostgreSQL role's (qd_grafana),
#                              random; tools/dev/grafana-role.sh sets it on
#                              the role, again after it is replaced
# It never overwrites a secret file and never prints a value. The folder is
# 700 and the files 444, as deploy/instalar.sh leaves them: a file secret is
# bind-mounted with the host file's owner and mode, and Grafana reads it as
# its own user (472), not as the file's owner.
set -eu
cd "$(dirname "$0")/../.."
set -a; . ./.env; set +a
dir=${QD_SECRETS_DIR:-./secrets}

# Created closed (umask 077), then given its mode explicitly: never the
# caller's umask.
umask 077
mkdir -p "$dir"
chmod 700 "$dir"

# write NAME VALUE: the file, only when it is not there yet.
write() {
  if [ -e "$dir/$1" ]; then
    echo "$1: kept"
  else
    printf '%s' "$2" >"$dir/$1"
    echo "$1: written"
  fi
  chmod 444 "$dir/$1"
}

if [ -n "${GRAFANA_ADMIN_PASSWORD:-}" ]; then
  write grafana_admin_password "$GRAFANA_ADMIN_PASSWORD"
else
  write grafana_admin_password "$(head -c 30 /dev/urandom | od -An -tx1 | tr -d ' \n')"
fi
if [ ! -s "$dir/grafana_admin_password" ]; then
  echo "grafana_admin_password is empty: remove it and run this again" >&2
  exit 1
fi

write grafana_pg_password "$(head -c 30 /dev/urandom | od -An -tx1 | tr -d ' \n')"

write grafana_metrics_password "$(head -c 30 /dev/urandom | od -An -tx1 | tr -d ' \n')"

# Empty means no SMTP authentication; a value given later fills it.
if [ -e "$dir/grafana_smtp_password" ] && [ ! -s "$dir/grafana_smtp_password" ] &&
  [ -n "${GRAFANA_SMTP_PASSWORD:-}" ]; then
  rm -f "$dir/grafana_smtp_password"
fi
write grafana_smtp_password "${GRAFANA_SMTP_PASSWORD:-}"

write grafana_oidc_client_secret "${GRAFANA_OIDC_CLIENT_SECRET:-}"
if [ ! -s "$dir/grafana_oidc_client_secret" ]; then
  cat <<EOF
grafana_oidc_client_secret is empty: sign-in with Quality Data will fail.
Register Grafana and write the secret it prints into the file:
  server oidc-client grafana Grafana https://<domain>/grafana/login/generic_oauth \\
    --home https://<domain>/grafana/ --detail "Auditoría y registros"
  chmod 600 $dir/grafana_oidc_client_secret   # write it, then chmod 444 again
then recreate Grafana (task obs:up).
EOF
fi
