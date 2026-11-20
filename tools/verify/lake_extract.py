# /// script
# requires-python = ">=3.11"
# dependencies = ["duckdb==2.0.0", "boto3"]
# ///
"""Verification for decision 0036: what an extraction needs, on synthetic
data only.

1. A source read through a driver: DuckDB (the worker's engine, the version
   the worker bundles) attaches a throwaway PostgreSQL holding generated
   rows, read-only, and sees its columns and types.
2. Writing a cut to bronze: DuckDB writes an Iceberg table through the
   warehouse's REST catalog; replacing a cut (delete its rows, insert them
   again) in one transaction, how many snapshots that leaves, and that
   doing it twice leaves the same rows (a retried job).
3. Credentials from STS limited to one bronze table: they write it, and
   cannot write another table of the same warehouse.

Only tables named `verify_*` are created; counts, ids and names are
printed, never a value, never a credential. From the repository root, with
the dev stack up and docker:

    uv run tools/verify/lake_extract.py
"""
import json, socket, subprocess, time
from pathlib import Path

import boto3, duckdb

ROOT = Path(__file__).resolve().parents[2]
env = dict(l.split("=", 1) for l in (ROOT / ".env").read_text().splitlines() if "=" in l and not l.startswith("#"))
ISSUER = (env["RUSTFS_STS_ISSUER_ACCESS_KEY"], env["RUSTFS_STS_ISSUER_SECRET_KEY"])
LOADER = (env["LAKE_LOADER_ACCESS_KEY"], env["LAKE_LOADER_SECRET_KEY"], None)
CA = str(ROOT / "infra/traefik/certs/dev-ca.crt")
BASE = "https://localhost"
W = "qd-tenant-00000000000000000000000000000001"
NS, OTHER = "verify_bronze", "verify_other_bronze"
CUT = "2026-10-31"
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


def assume(policy):
    sts = boto3.client("sts", endpoint_url=BASE, aws_access_key_id=ISSUER[0], aws_secret_access_key=ISSUER[1],
                       region_name="us-east-1", verify=CA)
    c = sts.assume_role(RoleArn="arn:aws:iam::000000000000:role/qd-job", RoleSessionName="extract-verify",
                        DurationSeconds=900, Policy=json.dumps(policy))["Credentials"]
    return c["AccessKeyId"], c["SecretAccessKey"], c["SessionToken"]


def lake(creds):
    ak, sk, tok = creds
    con = duckdb.connect()
    con.execute("INSTALL avro; LOAD avro; INSTALL iceberg; LOAD iceberg; INSTALL httpfs; LOAD httpfs;")
    con.execute(f"SET ca_cert_file='{CA}'")
    token = f", SESSION_TOKEN '{tok}'" if tok else ""
    con.execute(f"""CREATE SECRET s3 (TYPE s3, KEY_ID '{ak}', SECRET '{sk}'{token},
                    REGION 'us-east-1', ENDPOINT 'localhost', URL_STYLE 'path', USE_SSL true)""")
    con.execute(f"""ATTACH '{W}' AS lake (TYPE iceberg, URI '{BASE}/iceberg',
                    AUTHORIZATION_TYPE 'sigv4', SIGV4_SERVICE 's3', SIGV4_REGION 'us-east-1', SECRET s3)""")
    return con


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


@step("a throwaway PostgreSQL source with generated rows")
def _():
    C["port"] = free_port()
    C["container"] = subprocess.check_output([
        "docker", "run", "-d", "--rm", "-e", "POSTGRES_PASSWORD=verify", "-p", f"127.0.0.1:{C['port']}:5432",
        "postgres:16-alpine"], text=True).strip()
    for _ in range(60):
        if subprocess.run(["docker", "exec", C["container"], "pg_isready", "-U", "postgres"],
                          capture_output=True).returncode == 0:
            break
        time.sleep(1)
    time.sleep(2)
    sql = """CREATE TABLE clients (id integer PRIMARY KEY, document varchar(20), opened date, balance numeric(14,2));
             INSERT INTO clients SELECT g, 'D' || lpad(g::text, 8, '0'), DATE '2020-01-01' + g % 1000, g * 1.5
               FROM generate_series(1, 5000) g;
             CREATE ROLE reader LOGIN PASSWORD 'reader'; GRANT SELECT ON clients TO reader;"""
    subprocess.run(["docker", "exec", C["container"], "psql", "-qU", "postgres", "-c", sql], check=True,
                   capture_output=True)
    return "5000 generated rows; a read-only role"


@step("DuckDB reads it through its postgres driver, read-only: columns and types as seen")
def _():
    con = duckdb.connect()
    con.execute("INSTALL postgres; LOAD postgres;")
    con.execute(f"ATTACH 'host=127.0.0.1 port={C['port']} dbname=postgres user=reader password=reader' "
                "AS src (TYPE postgres, READ_ONLY)")
    cols = con.execute("SELECT column_name, data_type FROM duckdb_columns() WHERE database_name = 'src' "
                       "AND table_name = 'clients' ORDER BY column_index").fetchall()
    n = con.execute("SELECT count(*) FROM src.public.clients").fetchone()[0]
    try:
        con.execute("DELETE FROM src.public.clients")
        ro = "WROTE (unexpected)"
    except Exception as e:
        ro = f"a write is refused ({type(e).__name__})"
    C["src"] = con
    return f"{n} rows; {cols}; {ro}"


@step("setup (the platform's part): namespaces and tables created as the dev loader")
def _():
    con = lake(LOADER)
    for ns in (NS, OTHER):
        con.execute(f"CREATE SCHEMA IF NOT EXISTS lake.{ns}")
        con.execute(f"""CREATE TABLE IF NOT EXISTS lake.{ns}.clients
                        (id INTEGER, document VARCHAR, opened DATE, balance DECIMAL(14,2), cut DATE)""")
    C["loader"] = con
    return f"lake.{NS}.clients, lake.{OTHER}.clients"


def copy(con):
    """The source attached next to the lake, as the worker would do."""
    con.execute("INSTALL postgres; LOAD postgres;")
    con.execute(f"ATTACH 'host=127.0.0.1 port={C['port']} dbname=postgres user=reader password=reader' "
                "AS src (TYPE postgres, READ_ONLY)")
    con.execute("CREATE TEMP VIEW batch AS SELECT id, document, opened, balance FROM src.public.clients")


def replace_cut(con, table):
    con.execute("BEGIN")
    con.execute(f"DELETE FROM {table} WHERE cut = DATE '{CUT}'")
    con.execute(f"INSERT INTO {table} SELECT *, DATE '{CUT}' FROM batch")
    con.execute("COMMIT")


def snapshots(con, table):
    return con.execute(f"SELECT snapshot_id FROM iceberg_snapshots({table}) ORDER BY sequence_number").fetchall()


@step("a cut replaced in one transaction, twice: rows and snapshots each time")
def _():
    con, t = C["loader"], f"lake.{NS}.clients"
    copy(con)
    out = []
    for i in range(2):
        before = len(snapshots(con, t))
        replace_cut(con, t)
        n = con.execute(f"SELECT count(*) FROM {t} WHERE cut = DATE '{CUT}'").fetchone()[0]
        out.append(f"run {i + 1}: {n} rows, {len(snapshots(con, t)) - before} new snapshot(s)")
    files = con.execute(f"SELECT file_path FROM iceberg_metadata({t}) LIMIT 1").fetchone()[0]
    C["prefix"] = files.split(f"s3://{W}/", 1)[1].split("/data/", 1)[0]
    return "; ".join(out) + f"; files under {C['prefix']}/"


def one_table(prefix, ns, table):
    """Writes on the table's files (`tables/<uuid>/`) and on the catalog's
    own record of it (`.<…>/namespaces/<ns>/tables/<table>/`), which the
    commit checks; reads and catalog actions on the warehouse."""
    arns = [f"arn:aws:s3:::{W}", f"arn:aws:s3:::{W}/*"]
    return {"Version": "2012-10-17", "Statement": [
        {"Effect": "Allow", "Action": ["s3:GetBucketLocation", "s3:ListBucket", "s3:GetObject",
                                       "s3:ListBucketMultipartUploads"], "Resource": arns},
        {"Effect": "Allow", "Action": ["s3:PutObject", "s3:DeleteObject", "s3:AbortMultipartUpload",
                                       "s3:ListMultipartUploadParts"],
         "Resource": [f"arn:aws:s3:::{W}/{prefix}/*", f"arn:aws:s3:::{W}/.*/namespaces/{ns}/tables/{table}/*"]},
        {"Effect": "Allow", "Action": ["admin:GetTableCatalog", "admin:GetTableBucket", "admin:GetTableNamespace",
                                       "admin:GetTable", "admin:GetTableMetadata", "admin:GetTableMetadataLocation",
                                       "admin:CommitTable", "admin:SetTableMetadata",
                                       "admin:SetTableMetadataLocation"],
         "Resource": arns}]}


@step("STS limited to the table's files only (tables/<uuid>/): the commit is refused")
def _():
    policy = one_table(C["prefix"], NS, "clients")
    policy["Statement"][1]["Resource"] = policy["Statement"][1]["Resource"][:1]
    con = lake(assume(policy))
    copy(con)
    try:
        replace_cut(con, f"lake.{NS}.clients")
        return "WROTE (unexpected)"
    except Exception as e:
        return f"refused ({'403' if '403' in str(e) else str(e)[:120]})"


@step("STS limited to that table's files: replaces the cut")
def _():
    con = lake(assume(one_table(C["prefix"], NS, "clients")))
    copy(con)
    replace_cut(con, f"lake.{NS}.clients")
    C["scoped"] = con
    n = con.execute(f"SELECT count(*) FROM lake.{NS}.clients WHERE cut = DATE '{CUT}'").fetchone()[0]
    return f"{n} rows in the cut"


@step("STS limited to that table's files: cannot write another table")
def _():
    if "scoped" not in C:
        raise RuntimeError("no scoped connection")
    try:
        replace_cut(C["scoped"], f"lake.{OTHER}.clients")
        return "WROTE (unexpected)"
    except Exception as e:
        C["scoped"].execute("ROLLBACK") if "transaction" in str(e).lower() else None
        s = str(e)
        return f"denied ({type(e).__name__}: {'403' if '403' in s else 'AccessDenied' if 'AccessDenied' in s else s[:120]})"


@step("STS limited to that table's files: the other table is unchanged")
def _():
    return f"{C['loader'].execute(f'SELECT count(*) FROM lake.{OTHER}.clients').fetchone()[0]} rows"


@step("locked down after attaching both (0033): the source still reads, the lake still writes")
def _():
    con = lake(assume(one_table(C["prefix"], NS, "clients")))
    con.execute("INSTALL postgres; LOAD postgres;")
    con.execute(f"""CREATE SECRET src_job (TYPE postgres, HOST '127.0.0.1', PORT {C['port']},
                    DATABASE 'postgres', USER 'reader', PASSWORD 'reader')""")
    con.execute("ATTACH '' AS src (TYPE postgres, SECRET src_job, READ_ONLY)")
    con.execute(f"""SET allowed_directories = ['{BASE}/iceberg/v1/{W}/', 's3://{W}/'];
                    SET enable_external_access = false; SET lock_configuration = true;""")
    n = con.execute("SELECT count(*) FROM src.public.clients").fetchone()[0]
    con.execute("CREATE TEMP VIEW batch AS SELECT id, document, opened, balance FROM src.public.clients")
    replace_cut(con, f"lake.{NS}.clients")
    after = con.execute(f"SELECT count(*) FROM lake.{NS}.clients WHERE cut = DATE '{CUT}'").fetchone()[0]
    try:
        con.execute("ATTACH 'dbname=postgres' AS other (TYPE postgres)")
        later = "a later ATTACH WORKED (unexpected)"
    except Exception as e:
        later = f"a later ATTACH is refused ({type(e).__name__})"
    return f"source {n} rows read, cut {after} rows written; {later}"


def create_only(ns):
    """Creating a table in one namespace: the catalog's actions, and writes on
    the catalog's record of that namespace only, none on data files."""
    arns = [f"arn:aws:s3:::{W}", f"arn:aws:s3:::{W}/*"]
    return {"Version": "2012-10-17", "Statement": [
        {"Effect": "Allow", "Action": ["s3:GetBucketLocation", "s3:ListBucket", "s3:GetObject"], "Resource": arns},
        {"Effect": "Allow", "Action": ["s3:PutObject", "s3:DeleteObject"],
         "Resource": [f"arn:aws:s3:::{W}/.*/namespaces/{ns}/*"]},
        {"Effect": "Allow", "Action": ["admin:GetTableCatalog", "admin:GetTableBucket", "admin:GetTableNamespace",
                                       "admin:GetTable", "admin:GetTableMetadata", "admin:GetTableMetadataLocation",
                                       "admin:CreateTable", "admin:CommitTable", "admin:SetTableMetadata",
                                       "admin:SetTableMetadataLocation"], "Resource": arns}]}


@step("STS to create in one namespace: an empty table partitioned by cut, no data-file writes")
def _():
    con = lake(assume(create_only(NS)))
    t = f"created_{int(time.time())}"
    con.execute(f"CREATE TABLE lake.{NS}.{t} (id INTEGER, cut DATE) PARTITIONED BY (cut)")
    C["created"] = t
    return f"lake.{NS}.{t} created"


@step("the created table's files land by cut (partition applied, not ignored)")
def _():
    con, t = C["loader"], f"lake.{NS}.{C['created']}"
    con.execute(f"INSERT INTO {t} VALUES (1, DATE '2026-10-30'), (2, DATE '2026-10-31')")
    dirs = sorted({p.split("/data/", 1)[1].split("/", 1)[0] for (p,) in
                   con.execute(f"SELECT file_path FROM iceberg_metadata({t})").fetchall()})
    return f"data directories {dirs}"


@step("STS to create in one namespace: refused in another")
def _():
    con = lake(assume(create_only(NS)))
    try:
        con.execute(f"CREATE TABLE lake.{OTHER}.created_elsewhere (id INTEGER, cut DATE)")
        return "CREATED (unexpected)"
    except Exception as e:
        return f"refused ({'403' if '403' in str(e) else str(e)[:120]})"


@step("cleanup: drop the verify tables through the catalog (STS with admin:DeleteTable)")
def _():
    arns = [f"arn:aws:s3:::{W}", f"arn:aws:s3:::{W}/*"]
    con = lake(assume({"Version": "2012-10-17", "Statement": [
        {"Effect": "Allow", "Action": ["s3:*"], "Resource": arns},
        {"Effect": "Allow", "Action": ["admin:GetTableCatalog", "admin:GetTableBucket", "admin:GetTableNamespace",
                                       "admin:GetTable", "admin:GetTableMetadata", "admin:DeleteTable",
                                       "admin:DeleteTableNamespace"], "Resource": arns}]}))
    out = []
    for ns in (NS, OTHER):
        try:
            con.execute(f"DROP TABLE lake.{ns}.clients")
            out.append(f"{ns}.clients dropped")
        except Exception as e:
            out.append(f"{ns}.clients: {type(e).__name__}: {str(e)[-200:]}")
    return "; ".join(out)


if "container" in C:
    subprocess.run(["docker", "rm", "-f", C["container"]], capture_output=True)

for status, name, detail, secs in results:
    print(f"[{status:4}] {name} ({secs:.1f}s): {detail}")
