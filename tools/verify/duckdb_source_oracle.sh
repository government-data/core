#!/bin/bash
# Verification that DuckDB, at the engine the core pins, reads an Oracle
# source directly (decision 0077 step 3): a throwaway Oracle Database Free in
# a container, a table of invented rows and a user that may only select it;
# DuckDB attaches it through the community extension `oracle_scanner` (Oracle's
# TNS/TTC protocol spoken by the extension, no Oracle client), lists the
# schema's tables, reads the columns' types and reads rows with filters on
# decimals, dates and accented text. Run alone (no build at the same time),
# from anywhere:
#
#   bash tools/verify/duckdb_source_oracle.sh
#
# Needs docker and network access (the image, about 5 GB unpacked, the
# duckdb package, the extension). Oracle Free is held to 3 GB of memory.
# The passwords are made here and never printed. Everything is named
# qs-verify-oracle… and removed on exit, also on failure.
set -u
SRC=oracle
# shellcheck source=tools/verify/duckdb_source_lib.sh
. "$(dirname "$0")/duckdb_source_lib.sh"
ORA=qs-verify-oracle
CONTAINERS+=("$ORA")
IMAGE=${ORACLE_IMAGE:-gvenzl/oracle-free:23-slim-faststart}
ADMIN="Qs$(openssl rand -hex 12)"
APP="Qa$(openssl rand -hex 12)"
READER="Qr$(openssl rand -hex 12)"

sqlplus() { # USER PASSWORD, the SQL on stdin, against the pluggable database
  docker exec -i "$ORA" sqlplus -s -L "$1/\"$2\"@//localhost/FREEPDB1"
}

pinned_engine
net_up
echo "== Oracle in a container ($IMAGE)"
docker run -d --name "$ORA" --network "$NET" --memory 3g -e ORACLE_PASSWORD="$ADMIN" \
  -e APP_USER=qs_app -e APP_USER_PASSWORD="$APP" "$IMAGE" >/dev/null ||
  { ko "the container did not start"; exit 1; }
for _ in $(seq 100); do docker exec "$ORA" healthcheck.sh >/dev/null 2>&1 && break; sleep 3; done
docker exec "$ORA" healthcheck.sh >/dev/null 2>&1 || { ko "Oracle did not become ready"; exit 1; }
# shellcheck disable=SC2016 # v$version is Oracle's view, not a variable
ok "version: $(printf 'SET HEADING OFF FEEDBACK OFF LINESIZE 200\nSELECT banner_full FROM v$version;\n' |
  sqlplus system "$ADMIN" | tr -s '\n' ' ' | sed 's/^ *//')"
# Accented text written with UNISTR, so no client character set is involved.
sqlplus qs_app "$APP" >/dev/null <<SQL || { ko "the fixture"; exit 1; }
WHENEVER SQLERROR EXIT FAILURE
CREATE TABLE clients (id NUMBER(10) PRIMARY KEY, code VARCHAR2(10), name NVARCHAR2(60),
  opened DATE, amount NUMBER(12,2), updated TIMESTAMP);
INSERT INTO clients VALUES (1, '007', UNISTR('Caf\00E9 A\00F1ejo'), DATE '2020-01-15', 1500.50, TIMESTAMP '2026-10-01 08:00:00');
INSERT INTO clients VALUES (2, '120', UNISTR('Ping\00FCino Azul'), NULL, 0, TIMESTAMP '2026-10-02 09:30:00');
INSERT INTO clients VALUES (3, '045', UNISTR('\00C1rbol \00DAnico'), DATE '2031-12-31', -10.25, NULL);
INSERT INTO clients VALUES (4, '300', UNISTR('\00D1and\00FA Se\00F1orial'), DATE '2024-02-29', 987654321.99, TIMESTAMP '2026-11-20 23:59:59');
COMMIT;
SQL
sqlplus system "$ADMIN" >/dev/null <<SQL || { ko "the read-only user"; exit 1; }
WHENEVER SQLERROR EXIT FAILURE
CREATE USER qs_reader IDENTIFIED BY "$READER";
GRANT CREATE SESSION TO qs_reader;
GRANT SELECT ON qs_app.clients TO qs_reader;
SQL
ok "invented rows: $(printf 'SET HEADING OFF FEEDBACK OFF\nSELECT count(*) FROM clients;\n' | sqlplus qs_app "$APP" | tr -d ' \t\n')"

echo "== DuckDB reads it through the extension oracle_scanner"
duckdb_check -e EXT=oracle_scanner -e REPO=community -e SRC_PASSWORD="$READER" \
  -e SECRET_SQL="CREATE SECRET src_secret (TYPE oracle, HOST '$ORA', PORT 1521, SERVICE_NAME 'FREEPDB1', USER 'qs_reader', PASSWORD '@PASSWORD@')" \
  -e ATTACH_SQL="ATTACH 'src_secret' AS src (TYPE oracle_scanner, SCHEMA 'QS_APP', READ_ONLY)" \
  -e SCHEMA=main -e TABLE=src.main.clients \
  -e TYPES='{"id": ["INTEGER", "BIGINT", "DECIMAL(10,0)"], "code": ["VARCHAR"], "name": ["VARCHAR"], "opened": ["DATE", "TIMESTAMP"], "amount": ["DECIMAL(12,2)"], "updated": ["TIMESTAMP", "TIMESTAMP_NS"]}'

echo "== The extension from a directory, offline (decision 0077 point 8)"
ext_install oracle_scanner community
ext_offline oracle_scanner

verdict "Oracle"
