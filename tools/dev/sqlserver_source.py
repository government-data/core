# /// script
# requires-python = ">=3.11"
# dependencies = ["duckdb", "requests"]
# ///
"""The dev SQL Server source (decision 0048, Implementation step 6): the
stack of infra/sqlserver up, the database `qd_dev` with a read-only login,
each Parquet file of tmp/datos loaded into a table of `dbo` named after it,
and the source `sqlserver_dev` pointed at it, which the worker reads
through DuckDB (decision 0077) on 127.0.0.1, where the stack publishes it.

tmp/datos holds a cooperative's data and stays on this machine: it is read
by DuckDB and SQL Server only; this prints names, types and counts, never a
value. The passwords are made the first time and kept in .env, never
printed. Loading again replaces the tables.

From the repository root, with the dev stack and the API up:

    uv run tools/dev/sqlserver_source.py
"""
import os
import secrets
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import duckdb
import requests
import urllib3

urllib3.disable_warnings()
ROOT = Path(__file__).resolve().parents[2]
DATA = ROOT / "tmp" / "datos"
STACK = ["docker", "compose", "--project-directory", str(ROOT), "--env-file", str(ROOT / ".env"),
         "-p", "data-quality-sqlserver", "-f", str(ROOT / "infra/sqlserver/docker-compose.yml")]
CONTAINER, HOST, DATABASE, READER, SOURCE = (
    "data-quality-sqlserver-sqlserver-1", "127.0.0.1", "qd_dev", "qd_reader", "sqlserver_dev")
API = "https://localhost/api"


def env() -> dict[str, str]:
    lines = (ROOT / ".env").read_text().splitlines()
    return dict(l.split("=", 1) for l in lines if "=" in l and not l.startswith("#"))


def passwords() -> dict[str, str]:
    """SQLSERVER_SA_PASSWORD and SQLSERVER_READER_PASSWORD, made once."""
    values = env()
    missing = [k for k in ("SQLSERVER_SA_PASSWORD", "SQLSERVER_READER_PASSWORD") if not values.get(k)]
    if missing:
        with (ROOT / ".env").open("a") as f:
            f.write("\n# The dev SQL Server source (tools/dev/sqlserver_source.py)\n")
            for key in missing:
                values[key] = f"Qd-{secrets.token_hex(16)}!a1"
                f.write(f"{key}={values[key]}\n")
    return values


def sql(sa: str, query: str, database: str = "master") -> str:
    """Runs `query` as sa, the password in the environment, not the arguments."""
    out = subprocess.run(
        ["docker", "exec", "-e", "SQLCMDPASSWORD", CONTAINER, "/opt/mssql-tools18/bin/sqlcmd",
         "-C", "-b", "-S", "localhost", "-U", "sa", "-d", database, "-h", "-1", "-W", "-Q",
         f"SET NOCOUNT ON; {query}"],
        env={**os.environ, "SQLCMDPASSWORD": sa}, capture_output=True, text=True, check=False)
    if out.returncode != 0:
        # SQL Server's message names objects, never a row: safe to show.
        sys.exit(f"sqlserver: {out.stdout.strip()} {out.stderr.strip()}")
    return out.stdout.strip()


def bracket(name: str) -> str:
    return "[" + name.replace("]", "]]") + "]"


def mssql_type(duck: str) -> str:
    t = duck.upper()
    if t == "BOOLEAN":
        return "BIT"
    if t in ("TINYINT", "SMALLINT", "UTINYINT"):
        return "SMALLINT"
    if t in ("INTEGER", "USMALLINT"):
        return "INT"
    if t in ("BIGINT", "UINTEGER"):
        return "BIGINT"
    if t in ("HUGEINT", "UBIGINT"):
        return "DECIMAL(38,0)"
    if t == "FLOAT":
        return "REAL"
    if t == "DOUBLE":
        return "FLOAT"
    if t.startswith("DECIMAL"):
        return t
    if t == "DATE":
        return "DATE"
    if t.startswith("TIMESTAMP"):
        return "DATETIME2(6)"
    return "NVARCHAR(MAX)"


def as_csv(column: str, duck: str) -> str:
    """How DuckDB writes a column for SQL Server's BULK INSERT."""
    quoted = '"' + column.replace('"', '""') + '"'
    t = duck.upper()
    if t == "BOOLEAN":
        return f"CAST({quoted} AS INTEGER)"
    if t.startswith("TIMESTAMP"):
        return f"strftime(CAST({quoted} AS TIMESTAMP), '%Y-%m-%d %H:%M:%S.%f')"
    return quoted


def load(sa: str, parquet: Path, scratch: Path) -> None:
    table = parquet.stem
    db = duckdb.connect()
    columns = db.execute(f"DESCRIBE SELECT * FROM read_parquet('{parquet}')").fetchall()
    rows = db.execute(f"SELECT count(*) FROM read_parquet('{parquet}')").fetchone()[0]
    csv = scratch / f"{table}.csv"
    select = ", ".join(as_csv(name, duck) for name, duck, *_ in columns)
    db.execute(f"COPY (SELECT {select} FROM read_parquet('{parquet}')) TO '{csv}' "
               "(FORMAT csv, HEADER false, QUOTE '\"', ESCAPE '\"')")
    definition = ", ".join(f"{bracket(name)} {mssql_type(duck)} NULL" for name, duck, *_ in columns)
    sql(sa, f"DROP TABLE IF EXISTS dbo.{bracket(table)}; CREATE TABLE dbo.{bracket(table)} ({definition});",
        DATABASE)
    inside = f"/var/opt/mssql/load/{table}.csv"
    subprocess.run(["docker", "exec", "-u", "root", CONTAINER, "mkdir", "-p", "/var/opt/mssql/load"], check=True)
    # SQL Server on Linux reads no UTF-8 (no CODEPAGE): UTF-16, as `widechar`.
    wide = scratch / f"{table}.utf16.csv"
    with csv.open(encoding="utf-8", newline="") as src, wide.open("w", encoding="utf-16", newline="") as dst:
        shutil.copyfileobj(src, dst, 1 << 20)
    csv.unlink()
    subprocess.run(["docker", "cp", "-q", str(wide), f"{CONTAINER}:{inside}"], check=True)
    wide.unlink()
    subprocess.run(["docker", "exec", "-u", "root", CONTAINER, "chmod", "644", inside], check=True)
    try:
        sql(sa, f"BULK INSERT dbo.{bracket(table)} FROM '{inside}' WITH (FORMAT = 'CSV', "
                "FIELDQUOTE = '\"', DATAFILETYPE = 'widechar', ROWTERMINATOR = '\\n', TABLOCK);", DATABASE)
    finally:
        subprocess.run(["docker", "exec", "-u", "root", CONTAINER, "rm", "-f", inside], check=True)
    loaded = sql(sa, f"SELECT COUNT(*) FROM dbo.{bracket(table)};", DATABASE)
    kinds = sorted({mssql_type(duck) for _, duck, *_ in columns})
    print(f"  dbo.{table}: {len(columns)} columns ({', '.join(kinds)}), {rows} rows read, {loaded} loaded")
    if str(rows) != loaded:
        sys.exit(f"dbo.{table}: the counts differ")


def register(reader: str) -> None:
    values = env()
    s = requests.Session()
    s.verify = False
    s.post(f"{API}/auth/login", json={"email": values["ADMIN_BOOTSTRAP_EMAIL"],
                                       "password": values["ADMIN_BOOTSTRAP_PASSWORD"]})
    csrf = {"x-csrf-token": requests.utils.unquote(s.cookies.get("csrf", ""))}
    body = {"driver": "sqlserver",
            "fields": {"host": HOST, "port": "1433", "database": DATABASE,
                       "options": "encrypt=false"},
            "secrets": {"user": READER, "password": reader}}
    r = s.put(f"{API}/quality/sources/{SOURCE}", json=body, headers=csrf)
    print(f"source {SOURCE}: HTTP {r.status_code} (201 created, 204 saved again)")
    if r.status_code not in (201, 204):
        sys.exit(r.text[:200])


def main() -> None:
    files = sorted(DATA.glob("*.parquet"))
    if not files:
        sys.exit(f"no Parquet files in {DATA}")
    values = passwords()
    sa, reader = values["SQLSERVER_SA_PASSWORD"], values["SQLSERVER_READER_PASSWORD"]
    subprocess.run([*STACK, "up", "-d"], check=True, env={**os.environ, **values})
    for _ in range(60):
        probe = subprocess.run(
            ["docker", "exec", "-e", "SQLCMDPASSWORD", CONTAINER, "/opt/mssql-tools18/bin/sqlcmd",
             "-C", "-S", "localhost", "-U", "sa", "-Q", "SELECT 1"],
            env={**os.environ, "SQLCMDPASSWORD": sa}, capture_output=True, check=False)
        if probe.returncode == 0:
            break
        time.sleep(2)
    version = sql(sa, "SELECT CAST(SERVERPROPERTY('ProductVersion') AS varchar(20));")
    print(f"SQL Server {version}")
    quoted = reader.replace("'", "''")
    sql(sa, f"IF DB_ID('{DATABASE}') IS NULL CREATE DATABASE {DATABASE};")
    sql(sa, f"IF SUSER_ID('{READER}') IS NULL CREATE LOGIN {READER} WITH PASSWORD = '{quoted}' "
            f"ELSE ALTER LOGIN {READER} WITH PASSWORD = '{quoted}';")
    sql(sa, f"IF USER_ID('{READER}') IS NULL CREATE USER {READER} FOR LOGIN {READER}; "
            f"GRANT SELECT ON SCHEMA::dbo TO {READER};", DATABASE)
    scratch = Path(tempfile.mkdtemp(prefix="qd-sqlserver-"))
    try:
        print(f"{DATABASE}, read by {READER} (SELECT on dbo only):")
        for parquet in files:
            load(sa, parquet, scratch)
    finally:
        shutil.rmtree(scratch, ignore_errors=True)
    register(reader)


if __name__ == "__main__":
    main()
