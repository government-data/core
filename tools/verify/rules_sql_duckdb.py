# /// script
# requires-python = ">=3.11"
# dependencies = ["duckdb>=1.4"]
# ///
"""Verification for decision 0033, point 6, on DuckDB: what a rule's SQL
can reach, how literals are read, and the regular expression engine.
Synthetic values only; the lake check reads a count, never rows.

From the repository root, with the dev lake up and a bronze cut loaded
(tools/dev/lake-load.sh), so the lockdown is tried on a real attachment:

    set -a; . ./.env; set +a
    uv run tools/verify/rules_sql_duckdb.py

Without LAKE_LOADER_ACCESS_KEY the lake check is skipped."""
import os, time
from pathlib import Path

import duckdb

ROOT = Path(__file__).resolve().parents[2]
CA = str(ROOT / "infra/traefik/certs/dev-ca.crt")
results = []


def check(name, fn):
    t = time.time()
    try:
        out = fn()
        results.append(("OK  ", name, out, time.time() - t))
    except Exception as e:
        results.append(("FAIL", name, f"{type(e).__name__}: {str(e).splitlines()[0][:160]}", time.time() - t))


def refused(con, sql):
    """The statement must fail; returns the error's first line."""
    try:
        con.execute(sql).fetchall()
    except Exception as e:
        return f"refused: {type(e).__name__}: {str(e).splitlines()[0][:120]}"
    raise AssertionError(f"accepted: {sql}")


def one(con, sql):
    return con.execute(sql).fetchall()[0]


# --- Literals: the emitter doubles ' and writes everything else as is.
lit = duckdb.connect()
check("literal: a doubled quote is one quote", lambda: one(lit, "SELECT 'it''s' = 'it' || chr(39) || 's'"))
check("literal: a backslash is not an escape", lambda: one(lit, r"SELECT length('a\b'), 'a\b' = 'a' || chr(92) || 'b'"))
check("literal: Unicode quotes are plain characters",
      lambda: one(lit, "SELECT length('‘x’'), '’) OR 1=1 --' = chr(8217) || ') OR 1=1 --'"))
check("literal: a comment inside a literal stays text", lambda: one(lit, "SELECT length('-- x /* y */')"))
check("literal: NUL is accepted by DuckDB (the emitter must refuse it)",
      lambda: one(lit, "SELECT length('a" + "\x00" + "b')"))

# --- Regular expressions: RE2, anchored by the emitter.
check("regex: anchored match", lambda: one(lit, r"SELECT regexp_full_match('3001234567', '3\d{9}'), regexp_matches('x3001234567', '^(?:3\d{9})$')"))
check("regex: backreference refused (RE2)", lambda: refused(lit, r"SELECT regexp_matches('aa', '(a)\1')"))
check("regex: lookahead refused (RE2)", lambda: refused(lit, "SELECT regexp_matches('a', '(?=a)a')"))
check("regex: (a+)+ on 30 a's and a b is linear",
      lambda: one(lit, "SELECT regexp_matches(repeat('a', 30) || 'b', '^(?:(a+)+)$')"))

# --- The lockdown, on a connection that attached the lake first.
con = duckdb.connect()
lake = bool(os.environ.get("LAKE_LOADER_ACCESS_KEY"))
if lake:
    con.execute("INSTALL iceberg; LOAD iceberg; INSTALL httpfs; LOAD httpfs;")
    con.execute(f"SET ca_cert_file='{CA}'")
    con.execute(f"""CREATE SECRET s3 (TYPE s3, KEY_ID '{os.environ["LAKE_LOADER_ACCESS_KEY"]}',
                    SECRET '{os.environ["LAKE_LOADER_SECRET_KEY"]}', REGION 'us-east-1',
                    ENDPOINT 'localhost', URL_STYLE 'path', USE_SSL true)""")
    con.execute("""ATTACH 'qd-tenant-00000000000000000000000000000001' AS lake (TYPE iceberg, URI 'https://localhost/iceberg',
                   AUTHORIZATION_TYPE 'sigv4', SIGV4_SERVICE 's3', SIGV4_REGION 'us-east-1', SECRET s3)""")
    check("lake: count before the lockdown", lambda: one(con, "SELECT count(*) FROM lake.asesores_bronze.asesores"))
con.execute("CREATE TABLE t AS SELECT 1 AS id")
# Without this list the lockdown also cuts the attached lake: only the
# tenant's catalog and bucket stay reachable.
con.execute("SET allowed_directories = ['https://localhost/iceberg/v1/qd-tenant-00000000000000000000000000000001/', 's3://qd-tenant-00000000000000000000000000000001/']")
con.execute("SET enable_external_access = false")
con.execute("SET lock_configuration = true")
if lake:
    check("lake: an attached table is still read after the lockdown",
          lambda: one(con, "SELECT count(*) FROM lake.asesores_bronze.asesores"))
if lake:
    check("lockdown: another warehouse's bucket",
          lambda: refused(con, "SELECT * FROM read_parquet('s3://qd-tenant-other/x.parquet')"))
    check("lockdown: another warehouse's catalog",
          lambda: refused(con, "SELECT * FROM read_text('https://localhost/iceberg/v1/qd-tenant-other/namespaces')"))
check("lockdown: read_csv of a local file", lambda: refused(con, "SELECT * FROM read_csv('/etc/hostname')"))
check("lockdown: read_text", lambda: refused(con, "SELECT * FROM read_text('/etc/hostname')"))
check("lockdown: glob", lambda: refused(con, "SELECT * FROM glob('/etc/*')"))
check("lockdown: read_parquet over HTTP", lambda: refused(con, "SELECT * FROM read_parquet('https://localhost/x.parquet')"))
check("lockdown: COPY to a file", lambda: refused(con, "COPY t TO '/tmp/qd-verify-copy.csv'"))
check("lockdown: ATTACH a file", lambda: refused(con, "ATTACH '/tmp/qd-verify.db' AS x"))
check("lockdown: INSTALL an extension", lambda: refused(con, "INSTALL spatial"))
check("lockdown: turning it back on", lambda: refused(con, "SET enable_external_access = true"))
check("lockdown: another setting", lambda: refused(con, "SET memory_limit = '1GB'"))

print(f"DuckDB {duckdb.__version__}")
for status, name, out, secs in results:
    print(f"[{status}] {name}: {out} ({secs:.2f}s)")
failed = [r for r in results if r[0] == "FAIL"]
print("All checks passed." if not failed else f"{len(failed)} check(s) failed.")
raise SystemExit(1 if failed else 0)
