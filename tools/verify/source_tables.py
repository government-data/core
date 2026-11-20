# /// script
# requires-python = ">=3.11"
# dependencies = ["duckdb==2.0.0"]
# ///
"""Verification for decision 0042: what a source's listing can see through
DuckDB (the worker's engine, the version it bundles), on a throwaway
PostgreSQL and MySQL with synthetic tables only.

1. Tables and views of the attached source, by schema, with their column
   counts (`duckdb_tables`, `duckdb_views`, `duckdb_columns`), after the
   connection is locked down.
2. Whether the system's own schemas come in, and need filtering.
3. Whether a table the read-only user may not read is listed.

Names and counts only, never a value. From the repository root, with
docker:

    uv run tools/verify/source_tables.py
"""
import socket, subprocess, time

import duckdb

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


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


def start(image, env, inner, ready):
    port = free_port()
    args = ["docker", "run", "-d", "--rm", "-p", f"127.0.0.1:{port}:{inner}"]
    for k, v in env.items():
        args += ["-e", f"{k}={v}"]
    container = subprocess.check_output(args + [image], text=True).strip()
    for _ in range(90):
        if subprocess.run(["docker", "exec", container] + ready, capture_output=True).returncode == 0:
            break
        time.sleep(1)
    time.sleep(3)
    return container, port


def listing(con):
    """What a listing job reads: every table and view of `src`, its columns counted."""
    con.execute("SET enable_external_access = false; SET lock_configuration = true;")
    return con.execute("""
        SELECT schema_name, table_name, 'table' AS kind, column_count FROM duckdb_tables()
        WHERE database_name = 'src'
        UNION ALL
        SELECT v.schema_name, v.view_name, 'view',
               (SELECT count(*) FROM duckdb_columns() c WHERE c.database_name = 'src'
                AND c.schema_name = v.schema_name AND c.table_name = v.view_name)
        FROM duckdb_views() v WHERE v.database_name = 'src' AND NOT v.internal
        ORDER BY 1, 2""").fetchall()


@step("PostgreSQL 16: two schemas, a view, a table the reader may not read")
def _():
    C["pg"], port = start("postgres:16-alpine", {"POSTGRES_PASSWORD": "verify"}, 5432,
                          ["pg_isready", "-U", "postgres"])
    sql = """CREATE TABLE clients (id integer PRIMARY KEY, email text, opened date);
             CREATE SCHEMA loans; CREATE TABLE loans.credits (id integer, amount numeric(14,2));
             CREATE VIEW active_clients AS SELECT id, email FROM clients;
             CREATE TABLE secret_notes (id integer);
             CREATE ROLE reader LOGIN PASSWORD 'reader';
             GRANT SELECT ON clients, active_clients TO reader;
             GRANT USAGE ON SCHEMA loans TO reader; GRANT SELECT ON loans.credits TO reader;"""
    subprocess.run(["docker", "exec", C["pg"], "psql", "-qU", "postgres", "-c", sql], check=True,
                   capture_output=True)
    con = duckdb.connect()
    con.execute("INSTALL postgres; LOAD postgres;")
    con.execute(f"""CREATE SECRET src_job (TYPE postgres, HOST '127.0.0.1', PORT {port},
                    DATABASE 'postgres', USER 'reader', PASSWORD 'reader')""")
    con.execute("ATTACH '' AS src (TYPE postgres, SECRET src_job, READ_ONLY)")
    return str(listing(con))


@step("MySQL 8.4: a table, a view, a table the reader may not read")
def _():
    C["my"], port = start("mysql:8.4", {"MYSQL_ROOT_PASSWORD": "verify", "MYSQL_DATABASE": "core"}, 3306,
                          ["mysqladmin", "ping", "-uroot", "-pverify", "--silent"])
    sql = """CREATE TABLE core.clients (id int PRIMARY KEY, email text, opened date);
             CREATE VIEW core.active_clients AS SELECT id, email FROM core.clients;
             CREATE TABLE core.secret_notes (id int);
             CREATE USER 'reader'@'%' IDENTIFIED BY 'reader';
             GRANT SELECT ON core.clients TO 'reader'@'%';
             GRANT SELECT ON core.active_clients TO 'reader'@'%';"""
    for _ in range(30):
        if subprocess.run(["docker", "exec", C["my"], "mysql", "-uroot", "-pverify", "-e", sql],
                          capture_output=True).returncode == 0:
            break
        time.sleep(2)
    con = duckdb.connect()
    con.execute("INSTALL mysql; LOAD mysql;")
    con.execute(f"""CREATE SECRET src_job (TYPE mysql, HOST '127.0.0.1', PORT {port},
                    DATABASE 'core', USER 'reader', PASSWORD 'reader')""")
    con.execute("ATTACH '' AS src (TYPE mysql, SECRET src_job, READ_ONLY)")
    return str(listing(con))


for key in ("pg", "my"):
    if key in C:
        subprocess.run(["docker", "rm", "-f", C[key]], capture_output=True)

for status, name, detail, secs in results:
    print(f"[{status:4}] {name} ({secs:.1f}s): {detail}")
