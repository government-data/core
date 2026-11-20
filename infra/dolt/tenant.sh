#!/bin/sh
# A tenant's catalog database: created by root and granted to the
# application's user, which can then do everything inside it (tables,
# commits, branches, merges) and nothing outside it. See init/10-app-user.sh
# for why the application cannot create it itself.
#   infra/dolt/tenant.sh add <id>      CREATE DATABASE tenant_<id> + grants
#   infra/dolt/tenant.sh remove <id>   DROP DATABASE tenant_<id> + revoke
# From the repository root, with the stack up. `<id>`: [a-z0-9_], as the
# tenant's id in PostgreSQL.
# With the machines' certificates (DOLT_COMPOSE_FILES naming
# infra/dolt/certificate.yml, decision 0066) root presents the application
# VM's certificate, through that overlay's one-shot `sql`; DOLT_PROJECT
# (data-quality-dolt) and DOLT_ENV_FILE (.env) as backup.sh takes them.
set -eu
cd "$(dirname "$0")/../.."
action=${1:-} id=${2:-}
case $id in ''|*[!a-z0-9_]*) echo "usage: $0 add|remove <id>   (id: a-z 0-9 _)" >&2; exit 2 ;; esac
db="tenant_$id"
container=${DOLT_CONTAINER:-${APP_STACK:-data-quality}-dolt-dolt-1}
privs="SELECT, INSERT, UPDATE, DELETE, CREATE, DROP, ALTER, INDEX, REFERENCES, CREATE VIEW, SHOW VIEW, TRIGGER, EXECUTE, CREATE TEMPORARY TABLES, LOCK TABLES"
case $action in
  add) sql="CREATE DATABASE IF NOT EXISTS \`$db\`; GRANT $privs ON \`$db\`.* TO 'qd_app'@'%';" ;;
  remove) sql="DROP DATABASE IF EXISTS \`$db\`; REVOKE ALL ON \`$db\`.* FROM 'qd_app'@'%';" ;;
  *) echo "usage: $0 add|remove <id>" >&2; exit 2 ;;
esac
# Root, from inside the container's network: the only place it may connect from.
files=${DOLT_COMPOSE_FILES:-}
case $files in
  *certificate.yml*)
    # shellcheck disable=SC2086 # the files are words
    docker compose --project-directory . --env-file "${DOLT_ENV_FILE:-.env}" \
      -p "${DOLT_PROJECT:-data-quality-dolt}" $files --profile sql run --rm -T sql -e "$sql" >/dev/null ;;
  *) docker exec "$container" dolt sql -q "$sql" >/dev/null ;;
esac
echo "dolt: $db: $action done"
