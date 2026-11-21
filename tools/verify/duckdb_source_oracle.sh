#!/bin/bash
# Verification that DuckDB, at the engine the core pins, reads an Oracle
# source directly (decision 0077 step 3): a throwaway Oracle Database Free in
# a container, a table of invented rows and a user that may only select it;
# DuckDB attaches it through the community extension `oracle_scanner` (Oracle's
# TNS/TTC protocol spoken by the extension, no Oracle client), lists the
# schema's tables, reads the columns' types and reads rows with filters on
# decimals, dates and accented text. Then (step 4) the extension is loaded
# offline from an extension directory; on a session whose NLS_COMP is
# LINGUISTIC and NLS_SORT BINARY_CI (set by a logon trigger, as a DBA
# would), filters, DISTINCT, GROUP BY, joins and top-N run on a text column
# with the extension's pushdown on and off, against the user's rulings of
# 2026-11-21 (decision 0077 point 8): through the extension, filters,
# aggregates, DISTINCT and joins are DuckDB's, with aggregate and join
# pushdown off as the reader sets them. Last, how the extension
# lists and reads the connected user's tables and another owner's, with
# SCHEMA at ATTACH and without. Run alone (no build at the same time), from
# anywhere:
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
-- Step 4: the reader owns a table too, for the owner checks.
ALTER USER qs_reader QUOTA 10M ON users;
CREATE TABLE qs_reader.own_rows (id NUMBER(10) PRIMARY KEY);
INSERT INTO qs_reader.own_rows VALUES (7);
-- Step 4: a table of cased codes, and a user whose sessions compare text
-- ignoring case, by a logon trigger.
CREATE TABLE qs_app.cased (id NUMBER(10) PRIMARY KEY, code VARCHAR2(10));
INSERT INTO qs_app.cased VALUES (1, 'abc');
INSERT INTO qs_app.cased VALUES (2, 'ABC');
INSERT INTO qs_app.cased VALUES (3, 'Abc');
INSERT INTO qs_app.cased VALUES (4, 'xyz');
COMMIT;
CREATE USER qs_ci IDENTIFIED BY "$READER";
GRANT CREATE SESSION TO qs_ci;
GRANT SELECT ON qs_app.cased TO qs_ci;
CREATE OR REPLACE TRIGGER qs_ci_session AFTER LOGON ON qs_ci.SCHEMA
BEGIN
  EXECUTE IMMEDIATE 'ALTER SESSION SET NLS_COMP = LINGUISTIC';
  EXECUTE IMMEDIATE 'ALTER SESSION SET NLS_SORT = BINARY_CI';
END;
/
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

echo "== A case-insensitive session: filters, aggregates, DISTINCT and joins are DuckDB's"
# Each query with what DuckDB gives and what the session's BINARY_CI gives;
# the ruling wants DuckDB's, checked with aggregate and join pushdown off
# (what the reader sets; top-N pushdown stays on, as it changes nothing).
read -r -d '' CHECK2 <<'PY'
bad = []
def fail(m):  # goes on, so every case is seen; the run fails at the end
    bad.append(m); print(f"[FAIL] {m}", flush=True)
off = os.environ["PUSHDOWN"] == "off"
con.execute(os.environ["SECRET_SQL"].replace("@PASSWORD@", os.environ["SRC_PASSWORD"].replace("'", "''")))
if off:
    for s in ("oracle_aggregate_pushdown", "oracle_join_pushdown"):
        con.execute(f"SET {s} = false")
    ok("SET oracle_aggregate_pushdown, oracle_join_pushdown = false")
con.execute("ATTACH 'src_secret' AS src (TYPE oracle_scanner, SCHEMA 'QS_APP', READ_ONLY)")
oracle = lambda sql: con.execute("SELECT * FROM oracle_query('src_secret', '%s')" % sql.replace("'", "''")).fetchall()
nls = dict(oracle("SELECT parameter, value FROM nls_session_parameters WHERE parameter IN ('NLS_COMP', 'NLS_SORT')"))
ok(f"the extension's session: {nls}") if nls == {"NLS_COMP": "LINGUISTIC", "NLS_SORT": "BINARY_CI"} else fail(f"the extension's session: {nls}")
raw = oracle("SELECT count(*) FROM qs_app.cased WHERE code = 'abc'")[0][0]
ok(f"Oracle itself, code = 'abc': {raw} rows") if int(raw) == 3 else fail(f"Oracle itself, code = 'abc': {raw} rows, expected 3")
t = "src.main.cased"
SERVER, DUCK = "the server's BINARY_CI", "DuckDB's"
cases = [
    ("WHERE code = 'abc'", f"SELECT list(id ORDER BY id) FROM {t} WHERE code = 'abc'", [1], [1, 2, 3], DUCK),
    ("WHERE code LIKE 'a%'", f"SELECT list(id ORDER BY id) FROM {t} WHERE code LIKE 'a%'", [1], [1, 2, 3], DUCK),
    ("WHERE code IN ('abc', 'xyz')", f"SELECT list(id ORDER BY id) FROM {t} WHERE code IN ('abc', 'xyz')", [1, 4], [1, 2, 3, 4], DUCK),
    ("WHERE code > 'B'", f"SELECT list(id ORDER BY id) FROM {t} WHERE code > 'B'", [1, 4], [4], DUCK),
    ("count(DISTINCT code)", f"SELECT count(DISTINCT code) FROM {t}", 4, 2, DUCK),
    ("GROUP BY code", f"SELECT count(*) FROM (SELECT code FROM {t} GROUP BY code)", 4, 2, DUCK),
    ("a join on code", f"SELECT count(*) FROM {t} a JOIN {t} b ON a.code = b.code", 4, 10, DUCK),
    ("a join on id", f"SELECT count(*) FROM {t} a JOIN {t} b ON a.id = b.id", 4, 4, DUCK),
    ("top-N on id", f"SELECT list(id) FROM (SELECT id FROM {t} ORDER BY id DESC LIMIT 2)", [4, 3], [4, 3], DUCK),
    ("top-N on code", f"SELECT list(code) FROM (SELECT code FROM {t} ORDER BY code, id LIMIT 2)", ["ABC", "Abc"], ["abc", "ABC"], DUCK),
]
for name, sql, duck, server, ruled in cases:
    got = con.execute(sql).fetchone()[0]
    whose = "both" if duck == server == got else DUCK if got == duck else SERVER if got == server else "neither"
    line = f"{name}: {got!r}, {whose}"
    if not off:
        ok(line)
    else:
        ok(f"{line}, as ruled") if whose in (ruled, "both") else fail(f"{line}, the ruling wants {ruled}")
if bad:
    sys.exit(1)
PY
SECRET_CI="CREATE SECRET src_secret (TYPE oracle, HOST '$ORA', PORT 1521, SERVICE_NAME 'FREEPDB1', USER 'qs_ci', PASSWORD '@PASSWORD@')"
for pushdown in on off; do
  echo "-- the extension's pushdown of aggregates and joins: $pushdown"
  ext_check oracle_scanner -e PUSHDOWN=$pushdown -e SRC_PASSWORD="$READER" -e SECRET_SQL="$SECRET_CI"
done

echo "== Owners: the connected user's tables and another owner's"
# qs_reader owns OWN_ROWS and may select QS_APP.CLIENTS. For each ATTACH,
# what duckdb_tables() lists, before and after the reads, and which names
# read.
read -r -d '' CHECK2 <<'PY'
bad = []
def fail(m):
    bad.append(m); print(f"[FAIL] {m}", flush=True)
con.execute(os.environ["SECRET_SQL"].replace("@PASSWORD@", os.environ["SRC_PASSWORD"].replace("'", "''")))
names = ["src.own_rows", "src.main.own_rows", "src.QS_READER.own_rows", "src.qs_reader.own_rows",
         "src.clients", "src.main.clients", "src.QS_APP.clients", "src.qs_app.clients"]
# What the extension documents: main is the default owner (the connected
# user, or SCHEMA's), and every owner reads by its name.
expected = {
    "": {"src.main.own_rows", "src.QS_READER.own_rows", "src.QS_APP.clients"},
    "QS_APP": {"src.main.clients", "src.QS_APP.clients", "src.QS_READER.own_rows"},
    "QS_READER": {"src.main.own_rows", "src.QS_READER.own_rows", "src.QS_APP.clients"},
}
listed = lambda: [r[0] for r in con.execute(
    "SELECT schema_name || '.' || table_name FROM duckdb_tables() WHERE database_name = 'src' ORDER BY 1").fetchall()]
for schema in ["", "QS_APP", "QS_READER"]:
    option = f", SCHEMA '{schema}'" if schema else ""
    attach = f"ATTACH 'src_secret' AS src (TYPE oracle_scanner{option}, READ_ONLY)"
    con.execute(attach)
    ok(f"{attach}: duckdb_tables() lists {listed()}")
    ok("  duckdb_schemas(): %s" % [r[0] for r in con.execute(
        "SELECT schema_name FROM duckdb_schemas() WHERE database_name = 'src' ORDER BY 1").fetchall()])
    reads = set()
    for n in names:
        try:
            c = con.execute(f"SELECT count(*) FROM {n}").fetchone()[0]
            reads.add(n); ok(f"  {n} reads, {c} rows")
        except Exception as e:
            ok(f"  {n} does not read: {str(e).splitlines()[0][:100]}")
    missing = expected[schema] - reads
    fail(f"  expected to read {sorted(missing)}") if missing else ok(f"  reads {sorted(expected[schema])}, as documented")
    ok(f"  duckdb_tables() after the reads: {listed()}")
    con.execute("DETACH src")
if bad:
    sys.exit(1)
PY
SECRET="CREATE SECRET src_secret (TYPE oracle, HOST '$ORA', PORT 1521, SERVICE_NAME 'FREEPDB1', USER 'qs_reader', PASSWORD '@PASSWORD@')"
ext_check oracle_scanner -e SRC_PASSWORD="$READER" -e SECRET_SQL="$SECRET"

verdict "Oracle"
