# /// script
# requires-python = ">=3.11"
# dependencies = ["duckdb==2.0.0"]
# ///
"""Verification for decision 0038: what a map job can read of a source's
table through DuckDB (the worker's engine, the version it bundles), on a
throwaway PostgreSQL with generated rows only.

1. The table's metadata as DuckDB sees the attached source: columns and
   types, nullability, the primary key, a row estimate.
2. A sample's aggregates in one query, after the connection is locked down:
   empties (NULL or blank, U+00A0 too, decision 0033), distinct values,
   lengths, and how many values match each pattern of quality::proposal.
3. Whether `LIMIT` reads only the sample from the source, by the source's
   own statistics (inconclusive when DuckDB reads by `COPY`, which they
   may not count).

Counts and names only, never a value. From the repository root, with
docker:

    uv run tools/verify/source_map.py
"""
import socket, subprocess, time

import duckdb

# The defaults of quality::proposal::Patterns (crates/quality/src/proposal.rs).
PATTERNS = {
    "email": r"[^@\s]+@[^@\s]+\.[A-Za-z]{2,}",
    "mobile": r"3\d{9}",
    "document": r"\d{5,12}",
    "date": r"\d{4}-\d{2}-\d{2}.*",
    "number": r"-?\d+([.,]\d+)?",
    "flag": r"(?i)(s|n|si|no|y|yes|true|false|0|1)",
}
results, C = [], {}


def step(name):
    def wrap(fn):
        t = time.time()
        try:
            results.append(("OK", name, fn(), time.time() - t))
        except Exception as e:
            results.append(("FAIL", name, f"{type(e).__name__}: {str(e)[:300]}", time.time() - t))
        return fn
    return wrap


@step("a throwaway PostgreSQL with a synthetic table")
def _():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    C["port"] = s.getsockname()[1]
    s.close()
    C["container"] = subprocess.check_output([
        "docker", "run", "-d", "--rm", "-e", "POSTGRES_PASSWORD=verify", "-p", f"127.0.0.1:{C['port']}:5432",
        "postgres:16-alpine"], text=True).strip()
    for _ in range(60):
        if subprocess.run(["docker", "exec", C["container"], "pg_isready", "-U", "postgres"],
                          capture_output=True).returncode == 0:
            break
        time.sleep(1)
    time.sleep(2)
    sql = """CREATE TABLE clients (id integer PRIMARY KEY, document varchar(20) NOT NULL, email text,
               mobile varchar(12), opened date, balance numeric(14,2), active char(1));
             INSERT INTO clients SELECT g, lpad(g::text, 8, '0'),
               CASE WHEN g % 10 = 0 THEN NULL WHEN g % 10 = 1 THEN E'\\u00a0' ELSE 'c' || g || '@example.org' END,
               '3' || lpad(g::text, 9, '0'), DATE '2020-01-01' + g % 900, g * 1.5, CASE WHEN g % 2 = 0 THEN 'S' ELSE 'N' END
               FROM generate_series(1, 50000) g;
             ANALYZE clients;
             CREATE ROLE reader LOGIN PASSWORD 'reader'; GRANT SELECT ON clients TO reader;"""
    subprocess.run(["docker", "exec", C["container"], "psql", "-qU", "postgres", "-c", sql], check=True,
                   capture_output=True)
    return "50000 generated rows"


@step("attach read-only, then lock down: the source still reads")
def _():
    con = duckdb.connect()
    con.execute("INSTALL postgres; LOAD postgres;")
    con.execute(f"""CREATE SECRET src_job (TYPE postgres, HOST '127.0.0.1', PORT {C['port']},
                    DATABASE 'postgres', USER 'reader', PASSWORD 'reader')""")
    con.execute("ATTACH '' AS src (TYPE postgres, SECRET src_job, READ_ONLY)")
    con.execute("SET enable_external_access = false; SET lock_configuration = true;")
    C["con"] = con
    return f"{con.execute('SELECT count(*) FROM src.public.clients').fetchone()[0]} rows"


@step("metadata: columns, types, nullability (duckdb_columns)")
def _():
    rows = C["con"].execute("""SELECT column_name, data_type, is_nullable FROM duckdb_columns()
        WHERE database_name = 'src' AND schema_name = 'public' AND table_name = 'clients'
        ORDER BY column_index""").fetchall()
    return str(rows)


@step("metadata: the primary key (duckdb_constraints)")
def _():
    rows = C["con"].execute("""SELECT constraint_type, constraint_column_names FROM duckdb_constraints()
        WHERE database_name = 'src' AND table_name = 'clients'""").fetchall()
    return str(rows)


@step("metadata: a row estimate (duckdb_tables)")
def _():
    rows = C["con"].execute("""SELECT estimated_size FROM duckdb_tables()
        WHERE database_name = 'src' AND table_name = 'clients'""").fetchall()
    return str(rows)


@step("the sample's aggregates in one query, over LIMIT 10000")
def _():
    cols = ["id", "document", "email", "mobile", "opened", "balance", "active"]
    parts = []
    for c in cols:
        v = f'CAST("{c}" AS VARCHAR)'
        empty = f"({v} IS NULL OR regexp_full_match({v}, '[\\s\\x{{00A0}}]*'))"
        parts.append(f"count(*) FILTER (WHERE {empty})")
        parts.append(f'count(DISTINCT CASE WHEN NOT {empty} THEN "{c}" END)')
        parts.append(f"min(length({v})) FILTER (WHERE NOT {empty})")
        parts.append(f"max(length({v})) FILTER (WHERE NOT {empty})")
        for p in PATTERNS.values():
            parts.append(f"count(*) FILTER (WHERE regexp_full_match({v}, '{p}'))")
    sql = f"WITH s AS (SELECT * FROM src.public.clients LIMIT 10000) SELECT count(*), {', '.join(parts)} FROM s"
    row = C["con"].execute(sql).fetchone()
    width = 4 + len(PATTERNS)
    out = [f"sampled {row[0]}"]
    for i, c in enumerate(cols):
        r = row[1 + i * width: 1 + (i + 1) * width]
        looks = {k: n for k, n in zip(PATTERNS, r[4:]) if n}
        out.append(f"{c}: empty {r[0]}, distinct {r[1]}, length {r[2]}-{r[3]}, looks {looks}")
    return "; ".join(out)


@step("LIMIT and the source's own count of rows read (informative)")
def _():
    before = subprocess.check_output(["docker", "exec", C["container"], "psql", "-qtAU", "postgres", "-c",
        "SELECT coalesce(seq_tup_read, 0) FROM pg_stat_user_tables WHERE relname = 'clients'"], text=True).strip()
    C["con"].execute("SELECT count(*) FROM (SELECT * FROM src.public.clients LIMIT 10000)").fetchone()
    time.sleep(1)
    after = subprocess.check_output(["docker", "exec", C["container"], "psql", "-qtAU", "postgres", "-c",
        "SELECT coalesce(seq_tup_read, 0) FROM pg_stat_user_tables WHERE relname = 'clients'"], text=True).strip()
    return f"rows read by the source: {int(after) - int(before)}"


if "container" in C:
    subprocess.run(["docker", "rm", "-f", C["container"]], capture_output=True)

for status, name, detail, secs in results:
    print(f"[{status:4}] {name} ({secs:.1f}s): {detail}")
