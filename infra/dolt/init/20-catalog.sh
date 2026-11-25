#!/bin/bash
# The catalog's database (decisions 0021, 0076 point 5): one installation is
# one organisation, so it is the fixed name `quality::catalog::DATABASE`,
# created by root and granted to the application's user, as
# infra/dolt/tenant.sh add does; the tables are the application's, made on
# first use. Run by the image's entrypoint (or tls/entrypoint.sh) at the
# first start of an empty data volume, after 10-app-user.sh; idempotent.
set -eu
: "${DOLT_APP_USER:=qd_app}"
db=tenant_00000000000000000000000000000001
privs="SELECT, INSERT, UPDATE, DELETE, CREATE, DROP, ALTER, INDEX, REFERENCES, CREATE VIEW, SHOW VIEW, TRIGGER, EXECUTE, CREATE TEMPORARY TABLES, LOCK TABLES"
dolt sql <<SQL
CREATE DATABASE IF NOT EXISTS \`$db\`;
GRANT $privs ON \`$db\`.* TO '$DOLT_APP_USER'@'%';
SQL
echo "dolt: database $db ready for $DOLT_APP_USER"
