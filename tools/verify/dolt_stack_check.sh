#!/bin/sh
# Verification of infra/dolt (decision 0021): the stack as the application
# sees it, as `qd_app` over the published port. Each step prints OK or FAIL.
# Synthetic data only; it drops what it creates (`tenant_test`).
#
# From the repository root, with the stack up (task catalog:up) and, for the
# copy to the lake, infra/dolt/lake-backup-user.sh run once:
#   tools/verify/dolt_stack_check.sh
set -u
cd "$(dirname "$0")/../.."
set -a; . ./.env; set +a
host=127.0.0.1 port=${DOLT_PORT:-3307}
image=${DOLT_IMAGE:-dolthub/dolt-sql-server:2.6.1}
fails=0

# SQL on stdin, as qd_app, through a throwaway Dolt client on the host network.
app() {
  docker run --rm -i --network host -e DOLT_CLI_PASSWORD="$DOLT_APP_PASSWORD" --entrypoint dolt "$image" \
    --host "$host" --port "$port" --user qd_app --no-tls sql -r csv "$@" 2>&1
}
check() {  # name, expected (grep -E), output
  if printf '%s' "$3" | grep -qE "$2"; then
    echo "[OK  ] $1"
  else
    echo "[FAIL] $1: $(printf '%s' "$3" | tr '\n' ' ' | cut -c1-300)"; fails=$((fails + 1))
  fi
}

infra/dolt/tenant.sh remove test >/dev/null
out=$(infra/dolt/tenant.sh add test 2>&1)
check "root creates tenant_test and grants it to qd_app (tenant.sh)" 'add done' "$out"

out=$(app <<'SQL'
USE tenant_test;
CREATE TABLE rules (id VARCHAR(16) PRIMARY KEY, yaml TEXT, status VARCHAR(16));
INSERT INTO rules VALUES ('R-1', 'check: required', 'approved'), ('R-2', 'check: unique', 'approved');
CALL DOLT_COMMIT('-Am', 'catalog: first rules', '--author', 'Test <test@example.org>');
SELECT CONCAT('first=', HASHOF('main'));
SQL
)
check "qd_app creates a table and a commit in its tenant database" '^first=[0-9a-v]{32}' "$out"
first=$(printf '%s' "$out" | sed -n 's/^first=//p' | head -1)

out=$(app <<'SQL'
USE tenant_test;
CALL DOLT_BRANCH('draft-r1');
UPDATE `tenant_test/draft-r1`.rules SET yaml = 'check: required\nwhen: x > 0', status = 'draft' WHERE id = 'R-1';
USE `tenant_test/draft-r1`;
CALL DOLT_COMMIT('-am', 'draft: R-1', '--author', 'Test <test@example.org>');
USE tenant_test;
SELECT CONCAT('main=', yaml) FROM rules WHERE id = 'R-1';
SQL
)
check "a draft as a branch, committed, main untouched" '^main=check: required$' "$out"

out=$(app <<'SQL'
USE tenant_test;
SELECT CONCAT('diff=', diff_type, ':', from_status, '->', to_status) FROM DOLT_DIFF('main', 'draft-r1', 'rules');
SQL
)
check "diff of the draft against main" '^diff=modified:approved->draft' "$out"

out=$(app <<'SQL'
USE tenant_test;
CALL DOLT_MERGE('--no-ff', 'draft-r1', '-m', 'approve R-1', '--author', 'Approver <approver@example.org>');
SELECT CONCAT('merged=', status) FROM rules WHERE id = 'R-1';
SQL
)
check "approval as a merge into main" '^merged=draft' "$out"

out=$(app <<SQL
USE tenant_test;
SELECT CONCAT('asof=', status) FROM rules AS OF '$first' WHERE id = 'R-1';
SELECT CONCAT('log=', COUNT(*)) FROM dolt_log;
SQL
)
check "AS OF the first commit reads the old catalog" '^asof=approved' "$out"

out=$(app <<'SQL'
CREATE USER intruder IDENTIFIED BY 'x';
SQL
)
check "qd_app cannot CREATE USER" '[Aa]ccess denied|command denied|not allowed|privilege' "$out"

out=$(app <<'SQL'
CREATE DATABASE tenant_other;
SQL
)
check "qd_app cannot create databases" '[Aa]ccess denied|command denied|privilege' "$out"

out=$(app <<'SQL'
SELECT CONCAT('users=', COUNT(*)) FROM mysql.user;
SQL
)
check "qd_app cannot read the users table" '[Aa]ccess denied|command denied|privilege|not found' "$out"

out=$(app <<'SQL'
INSERT INTO mysql.user (Host, User) VALUES ('%', 'intruder');
SQL
)
check "qd_app cannot write the users table" '[Aa]ccess denied|command denied|privilege|not found' "$out"

out=$(app <<'SQL'
GRANT SELECT ON *.* TO qd_app;
SQL
)
check "qd_app cannot grant" '[Aa]ccess denied|command denied|privilege|GRANT OPTION' "$out"

out=$(infra/dolt/backup.sh 2>&1)
check "backup of tenant_test into the volume" 'tenant_test backed up' "$out"
if [ -n "${DOLT_BACKUP_S3_ACCESS_KEY:-}" ]; then
  check "backups copied to the lake" 'copied to s3://' "$out"
  out=$(docker run --rm --network "container:${APP_STACK:-data-quality}-traefik-1" \
    -e RCLONE_CONFIG_LAKE_TYPE=s3 -e RCLONE_CONFIG_LAKE_PROVIDER=Other \
    -e RCLONE_CONFIG_LAKE_ENDPOINT="${DOLT_BACKUP_S3_URL:-http://127.0.0.1:9000}" \
    -e RCLONE_CONFIG_LAKE_FORCE_PATH_STYLE=true \
    -e RCLONE_CONFIG_LAKE_ACCESS_KEY_ID="$DOLT_BACKUP_S3_ACCESS_KEY" \
    -e RCLONE_CONFIG_LAKE_SECRET_ACCESS_KEY="$DOLT_BACKUP_S3_SECRET_KEY" \
    "${RCLONE_IMAGE:-rclone/rclone:1.75.2}" lsf "lake:${DOLT_BACKUP_BUCKET:-qd-backups}/dolt/tenant_test/" 2>&1)
  check "the backup is in the bucket" 'manifest' "$out"
  out=$(docker run --rm --network "container:${APP_STACK:-data-quality}-traefik-1" \
    -e RCLONE_CONFIG_LAKE_TYPE=s3 -e RCLONE_CONFIG_LAKE_PROVIDER=Other \
    -e RCLONE_CONFIG_LAKE_ENDPOINT="${DOLT_BACKUP_S3_URL:-http://127.0.0.1:9000}" \
    -e RCLONE_CONFIG_LAKE_FORCE_PATH_STYLE=true -e RCLONE_CONFIG_LAKE_NO_CHECK_BUCKET=true \
    -e RCLONE_CONFIG_LAKE_ACCESS_KEY_ID="$DOLT_BACKUP_S3_ACCESS_KEY" \
    -e RCLONE_CONFIG_LAKE_SECRET_ACCESS_KEY="$DOLT_BACKUP_S3_SECRET_KEY" \
    "${RCLONE_IMAGE:-rclone/rclone:1.75.2}" touch "lake:${DOLT_BACKUP_BUCKET:-qd-backups}/elsewhere.txt" 2>&1)
  check "the backup user cannot write outside dolt/" 'AccessDenied|403|denied' "$out"
fi

out=$(docker exec "${APP_STACK:-data-quality}-dolt-dolt-1" dolt sql -r csv -q \
  "CALL DOLT_BACKUP('restore', 'file:///backups/tenant_test', 'tenant_test_restored'); USE tenant_test_restored; SELECT CONCAT('restored=', COUNT(*)) FROM dolt_log; DROP DATABASE tenant_test_restored;" 2>&1)
check "restore of the backup into another database" '^restored=[0-9]+' "$out"

infra/dolt/tenant.sh remove test >/dev/null
out=$(docker exec "${APP_STACK:-data-quality}-dolt-dolt-1" dolt sql -r csv -q \
  "SELECT CONCAT('left=', COUNT(*)) FROM information_schema.schemata WHERE schema_name LIKE 'tenant_test%'" 2>&1)
check "tenant_test dropped (tenant.sh)" '^left=0' "$out"
# Its backup goes too, locally and, at the next copy, in the bucket.
docker exec "${APP_STACK:-data-quality}-dolt-dolt-1" rm -rf /backups/tenant_test
infra/dolt/backup.sh >/dev/null 2>&1

echo "failures: $fails"
[ "$fails" = 0 ]
