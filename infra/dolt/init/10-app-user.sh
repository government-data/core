#!/bin/bash
# The application's user (decision 0021: only the application talks to
# Dolt). Run by the image's entrypoint at the first start of an empty data
# volume; idempotent, so it can also be run again by hand to apply a new
# password from .env:
#   docker exec data-quality-dolt-dolt-1 /docker-entrypoint-initdb.d/10-app-user.sh
# The user starts with no database at all. Dolt 2.6 has no wildcard grants
# (`tenant\_%`.* matches nothing) and CREATE DATABASE needs a global grant,
# which in Dolt also opens the `mysql` database (users, grants) to writes;
# so the catalog database is created by root and granted to this user,
# branches included (`tenant_<id>/<branch>`): infra/dolt/tenant.sh.
set -eu
: "${DOLT_APP_USER:=qd_app}"
: "${DOLT_APP_PASSWORD:?set DOLT_APP_PASSWORD in .env}"
pw=$(printf %s "$DOLT_APP_PASSWORD" | sed "s/'/''/g")
dolt sql <<SQL
CREATE USER IF NOT EXISTS '$DOLT_APP_USER'@'%' IDENTIFIED BY '$pw';
ALTER USER '$DOLT_APP_USER'@'%' IDENTIFIED BY '$pw';
SQL
echo "dolt: user $DOLT_APP_USER ready"
