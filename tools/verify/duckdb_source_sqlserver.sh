#!/bin/bash
# Verification that DuckDB, at the engine the core pins, reads a SQL Server
# source directly (decision 0077 step 3): a throwaway SQL Server in a
# container, a table of invented rows and a read-only login; DuckDB attaches
# it through the community extension `mssql` (native TDS, no ODBC driver),
# lists the schema's tables, reads the columns' types and reads rows with
# filters on decimals, dates and accented text. Then (step 4) the extension
# is loaded offline from an extension directory, and a case-sensitive filter,
# DISTINCT, GROUP BY and a join run on a case-insensitive column, attached
# with and without `remote_pushdown false`: with it, each must give DuckDB's
# answer, not SQL Server's collation's. Run alone (no build at the
# same time), from anywhere:
#
#   bash tools/verify/duckdb_source_sqlserver.sh
#
# Needs docker and network access (the image, the duckdb package, the
# extension). The passwords are made here and never printed. Everything is
# named qs-verify-mssql… and removed on exit, also on failure.
set -u
SRC=mssql
# shellcheck source=tools/verify/duckdb_source_lib.sh
. "$(dirname "$0")/duckdb_source_lib.sh"
MSSQL=qs-verify-mssql
CONTAINERS+=("$MSSQL")
# The image and settings of infra/sqlserver/docker-compose.yml.
IMAGE=${MSSQL_IMAGE:-mcr.microsoft.com/mssql/server:2022-latest}
SA="Qs-$(openssl rand -hex 12)!A1"
READER="Qr-$(openssl rand -hex 12)!B2"

sqlserver() { # SQL, as sa, with UTF-8 input
  docker exec -e SQLCMDPASSWORD="$SA" "$MSSQL" /opt/mssql-tools18/bin/sqlcmd -C -f 65001 \
    -S localhost -U sa -b -h -1 -W -Q "SET NOCOUNT ON; $1"
}

pinned_engine
net_up
echo "== SQL Server in a container ($IMAGE)"
docker run -d --name "$MSSQL" --network "$NET" -e ACCEPT_EULA=Y -e MSSQL_PID=Developer \
  -e MSSQL_MEMORY_LIMIT_MB=2048 -e MSSQL_SA_PASSWORD="$SA" "$IMAGE" >/dev/null ||
  { ko "the container did not start"; exit 1; }
for _ in $(seq 60); do sqlserver "SELECT 1" >/dev/null 2>&1 && break; sleep 2; done
[ "$(sqlserver 'SELECT 1' 2>&1 | tr -d '\r')" = 1 ] || { ko "SQL Server did not answer"; exit 1; }
ok "version: $(sqlserver "SELECT CAST(SERVERPROPERTY('ProductVersion') AS varchar(20)) + ' ' + CAST(SERVERPROPERTY('Edition') AS varchar(60))" | tr -d '\r')"
sqlserver "CREATE DATABASE qs_verify" >/dev/null || { ko "create database"; exit 1; }
sqlserver "USE qs_verify;
  CREATE TABLE dbo.clients (id INT PRIMARY KEY, code VARCHAR(10), name NVARCHAR(60), opened DATE,
    amount DECIMAL(12,2), updated DATETIME2);
  INSERT INTO dbo.clients VALUES
    (1, '007', N'Café Añejo', '2020-01-15', 1500.50, '2026-10-01T08:00:00'),
    (2, '120', N'Pingüino Azul', NULL, 0, '2026-10-02T09:30:00'),
    (3, '045', N'Árbol Único', '2031-12-31', -10.25, NULL),
    (4, '300', N'Ñandú Señorial', '2024-02-29', 987654321.99, '2026-11-20T23:59:59');
  CREATE TABLE dbo.cased (id INT PRIMARY KEY, code VARCHAR(10));
  INSERT INTO dbo.cased VALUES (1, 'abc'), (2, 'ABC'), (3, 'Abc'), (4, 'xyz');
  CREATE LOGIN qs_reader WITH PASSWORD = '$READER';
  CREATE USER qs_reader FOR LOGIN qs_reader;
  GRANT SELECT ON SCHEMA::dbo TO qs_reader;" >/dev/null || { ko "the fixture"; exit 1; }
ok "invented rows: $(sqlserver 'SELECT count(*) FROM qs_verify.dbo.clients' | tr -d '\r')"

echo "== DuckDB reads it through the extension mssql"
duckdb_check -e EXT=mssql -e REPO=community -e SRC_PASSWORD="$READER" \
  -e SECRET_SQL="CREATE SECRET src_secret (TYPE mssql, host '$MSSQL', port 1433, database 'qs_verify', user 'qs_reader', password '@PASSWORD@', trust_server_certificate true)" \
  -e ATTACH_SQL="ATTACH '' AS src (TYPE mssql, SECRET src_secret, READ_ONLY)" \
  -e SCHEMA=dbo -e TABLE=src.dbo.clients \
  -e TYPES='{"id": ["INTEGER"], "code": ["VARCHAR"], "name": ["VARCHAR"], "opened": ["DATE"], "amount": ["DECIMAL(12,2)"], "updated": ["TIMESTAMP", "TIMESTAMP_NS"]}'

echo "== The extension from a directory, offline (decision 0077 point 8)"
ext_install mssql community
ext_offline mssql

echo "== Case-sensitive filters, with and without remote_pushdown false"
ok "collation of dbo.cased.code: $(sqlserver "SELECT collation_name FROM qs_verify.sys.columns WHERE object_id = OBJECT_ID('qs_verify.dbo.cased') AND name = 'code'" | tr -d '\r')"
# Each query with what DuckDB gives and what SQL Server's case-insensitive
# collation gives; the casts point 8 makes into bronze.
read -r -d '' CHECK2 <<'PY'
import datetime as dt
with_option = os.environ["OPTION"] == "on"
con.execute(os.environ["SECRET_SQL"].replace("@PASSWORD@", os.environ["SRC_PASSWORD"].replace("'", "''")))
attach = "ATTACH '' AS src (TYPE mssql, SECRET src_secret, READ_ONLY%s)" % (", remote_pushdown false" if with_option else "")
con.execute(attach)
ok(f"attached: {attach}")
t = "src.dbo.cased"
cases = [
    ("WHERE code = 'abc'", f"SELECT list(id ORDER BY id) FROM {t} WHERE code = 'abc'", [1], [1, 2, 3]),
    ("WHERE code LIKE 'a%'", f"SELECT list(id ORDER BY id) FROM {t} WHERE code LIKE 'a%'", [1], [1, 2, 3]),
    ("WHERE code IN ('abc', 'xyz')", f"SELECT list(id ORDER BY id) FROM {t} WHERE code IN ('abc', 'xyz')", [1, 4], [1, 2, 3, 4]),
    ("count(DISTINCT code)", f"SELECT count(DISTINCT code) FROM {t}", 4, 2),
    ("GROUP BY code", f"SELECT count(*) FROM (SELECT code FROM {t} GROUP BY code)", 4, 2),
    ("a join on code", f"SELECT count(*) FROM {t} a JOIN {t} b ON a.code = b.code", 4, 10),
]
server, wrong = [], []
for name, sql, duck, collation in cases:
    got = con.execute(sql).fetchone()[0]
    whose = "DuckDB's" if got == duck else "SQL Server's collation" if got == collation else "neither"
    line = f"{name}: {got!r}, {whose}"
    if whose == "SQL Server's collation":
        server.append(name)
    if with_option and whose != "DuckDB's":
        wrong.append(name)
        print(f"[FAIL] {line}", flush=True)
    else:
        ok(line)
if not with_option:
    ok(f"without the option, SQL Server's collation decides: {server}") if server else ok("without the option, every case is DuckDB's")
con.execute("""CREATE TEMP TABLE bronze AS SELECT CAST(code AS VARCHAR) AS code, CAST(name AS VARCHAR) AS name,
    CAST(updated AS TIMESTAMP) AS updated FROM src.dbo.clients""")
types = [r[1] for r in con.execute("DESCRIBE bronze").fetchall()]
ok(f"cast into bronze: {types}") if types == ["VARCHAR", "VARCHAR", "TIMESTAMP"] else ko(f"cast into bronze: {types}")
got = con.execute("SELECT updated FROM bronze WHERE code = '300'").fetchone()[0]
ok(f"DATETIME2 as TIMESTAMP: {got}") if got == dt.datetime(2026, 11, 20, 23, 59, 59) else ko(f"DATETIME2 as TIMESTAMP: {got}")
if wrong:
    ko(f"with the option, SQL Server's collation still decides: {wrong}")
PY
SECRET="CREATE SECRET src_secret (TYPE mssql, host '$MSSQL', port 1433, database 'qs_verify', user 'qs_reader', password '@PASSWORD@', trust_server_certificate true)"
for option in off on; do
  echo "-- remote_pushdown false: $option"
  ext_check mssql -e OPTION=$option -e SRC_PASSWORD="$READER" -e SECRET_SQL="$SECRET"
done

verdict "SQL Server"
